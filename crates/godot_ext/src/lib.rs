//! Minimal Godot 4 bridge for Phase 0.
//!
//! The bridge deliberately exposes a coarse API: it builds meshes in the
//! engine-agnostic Rust core and hands them to Godot as `ArrayMesh` resources.
//! Terrain generation and meshing run on Rust worker threads; all Godot
//! resource creation stays on the main thread.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use godot::builtin::{
    PackedColorArray, PackedFloat32Array, PackedInt32Array, PackedVector2Array, PackedVector3Array,
    VarArray, Variant,
};
use godot::classes::mesh::{ArrayType, PrimitiveType};
use godot::classes::{ArrayMesh, SurfaceTool};
use godot::prelude::*;

use core_blueprint::{
    load_tsbp, save_tsbp_with_hash, Blueprint, BlueprintError, BlueprintMetadata, BlueprintStats,
    PropRecord, RoomRecord,
};
use core_city::{
    normalize_rotation, summarize, CityLayout, CityRuntime, CitySummary, CityTier,
    ConstructionSite, ConstructionStore, Economy, Home, IndustryKind, IndustryStore, Lot, LotError,
    LotRequest, LotStore, Placement, PlacementError, PlacementRequest, PlacementStore, RoadError,
    RoadRequest, RoadSegment, RoadStore, Settlement, SettlementError, SettlementRequest,
    SettlementStore, Workplace, Zone, MAX_WORLD_COORD_M,
};
use core_rooms::{
    count_materials, demo as room_demo, detect_rooms, PropInput, PropKind, RoomBounds,
    RoomMaterials, RoomStats, FLOOR_BAND_M,
};
use core_voxel::coords::{MaterialId, AIR, CHUNK_SIZE, VOXEL_SIZE_M};
use core_voxel::demo;
use core_voxel::mesh::{greedy_mesh, MeshBuffers};
use core_voxel::volume::{VolumeConfig, VolumePos, VoxelVolume, MAX_CHUNKS, MAX_CHUNKS_PER_AXIS};
use core_world::{
    terrain_mesh_lod_with_sampler_and_skirt, DeltaStore, TerrainMesh, TerrainPos, WorldChunkCoord,
    WorldGenerator, DEFAULT_SKIRT_DEPTH_M, MAX_HEIGHT_M, WORLD_CHUNK_SIZE,
};

const MAX_TERRAIN_WORKERS: usize = 4;

/// Material ids used by the Phase 1 editor palette and room detector.
const EDITOR_WALL: u16 = 1;
const EDITOR_FLOOR: u16 = 2;
const EDITOR_CEILING: u16 = 3;
const EDITOR_WINDOW: u16 = 4;
const EDITOR_DOOR: u16 = 5;

/// Placeholder slice prices in abstract cost units per litre.
///
/// The vertical slice has no economy yet, so these values only make the cost
/// stat and `.tsbp` metadata meaningful and deterministic. Unknown/modded
/// materials fall back to `1.0` per litre. See `docs/26` and `docs/03`.
const MATERIAL_COST_PER_LITER: [(MaterialId, f64); 5] = [
    (EDITOR_WALL, 0.4),
    (EDITOR_FLOOR, 0.9),
    (EDITOR_CEILING, 0.7),
    (EDITOR_WINDOW, 1.8),
    (EDITOR_DOOR, 2.5),
];

/// Price the `material:{id}` counts returned by [`count_materials`].
fn slice_material_cost(materials: &BTreeMap<String, u64>) -> f64 {
    materials
        .iter()
        .map(|(key, count)| {
            let id = key
                .strip_prefix("material:")
                .and_then(|value| value.parse::<MaterialId>().ok());
            let price = id
                .and_then(|id| {
                    MATERIAL_COST_PER_LITER
                        .iter()
                        .find(|(material, _)| *material == id)
                        .map(|(_, price)| *price)
                })
                .unwrap_or(1.0);
            price * *count as f64
        })
        .sum()
}

fn editor_room_materials() -> RoomMaterials {
    RoomMaterials::new(
        EDITOR_WALL,
        EDITOR_FLOOR,
        EDITOR_CEILING,
        EDITOR_DOOR,
        EDITOR_WINDOW,
    )
}

fn prop_kind_from_code(code: i64) -> Option<PropKind> {
    match code {
        0 => Some(PropKind::Bed),
        1 => Some(PropKind::Stove),
        2 => Some(PropKind::Toilet),
        3 => Some(PropKind::Shower),
        4 => Some(PropKind::Sink),
        5 => Some(PropKind::Table),
        6 => Some(PropKind::Sofa),
        7 => Some(PropKind::Desk),
        8 => Some(PropKind::Other),
        _ => None,
    }
}

fn prop_code(kind: PropKind) -> i64 {
    match kind {
        PropKind::Bed => 0,
        PropKind::Stove => 1,
        PropKind::Toilet => 2,
        PropKind::Shower => 3,
        PropKind::Sink => 4,
        PropKind::Table => 5,
        PropKind::Sofa => 6,
        PropKind::Desk => 7,
        PropKind::Other => 8,
    }
}

fn prop_id(kind: PropKind) -> &'static str {
    match kind {
        PropKind::Bed => "base:bed_single",
        PropKind::Stove => "base:stove",
        PropKind::Toilet => "base:toilet",
        PropKind::Shower => "base:shower",
        PropKind::Sink => "base:sink",
        PropKind::Table => "base:table",
        PropKind::Sofa => "base:sofa",
        PropKind::Desk => "base:desk",
        PropKind::Other => "base:prop",
    }
}

fn prop_kind_from_id(id: &str) -> Option<PropKind> {
    match id {
        "base:bed_single" | "base:bed_double" => Some(PropKind::Bed),
        "base:stove" | "base:oven" => Some(PropKind::Stove),
        "base:toilet" => Some(PropKind::Toilet),
        "base:shower" => Some(PropKind::Shower),
        "base:sink" => Some(PropKind::Sink),
        "base:table" => Some(PropKind::Table),
        "base:sofa" => Some(PropKind::Sofa),
        "base:desk" => Some(PropKind::Desk),
        "base:prop" => Some(PropKind::Other),
        _ => None,
    }
}

fn prop_input(prop: &EditorProp) -> PropInput {
    PropInput::new(
        prop_id(prop.kind),
        prop.kind,
        [
            (prop.cell[0] as f32 + 0.5) * VOXEL_SIZE_M,
            prop.cell[1] as f32 * VOXEL_SIZE_M,
            (prop.cell[2] as f32 + 0.5) * VOXEL_SIZE_M,
        ],
    )
}

fn editor_props_input(props: &[EditorProp]) -> Vec<PropInput> {
    props.iter().map(prop_input).collect()
}

/// Build blueprint metadata from the editor volume, including detected rooms.
fn blueprint_from_editor_volume(
    id: String,
    name: String,
    volume: VoxelVolume,
    props: &[EditorProp],
) -> Blueprint {
    let inputs = editor_props_input(props);
    let stats = detect_rooms(&volume, &editor_room_materials(), &inputs);
    let mut metadata = BlueprintMetadata::new(id, name);
    metadata.materials = stats.materials.clone();
    metadata.rooms = stats
        .rooms
        .iter()
        .map(|room| RoomRecord {
            id: room.id.clone(),
            room_type: room.room_type.clone(),
            floor: room.floor,
            area_m2: room.area_m2,
            volume_m3: room.volume_m3,
            beds: room.beds,
            doors: room.doors,
            windows: room.windows,
        })
        .collect();
    metadata.stats = BlueprintStats {
        resident_capacity: stats.resident_capacity,
        job_slots: stats.job_slots,
        class: stats.class.clone(),
        material_cost: slice_material_cost(&stats.materials),
    };
    let mut ordered: Vec<&EditorProp> = props.iter().collect();
    ordered.sort_by_key(|prop| (prop.cell[1], prop.cell[2], prop.cell[0]));
    metadata.props = ordered
        .into_iter()
        .map(|prop| PropRecord {
            id: prop_id(prop.kind).to_string(),
            position: [
                (prop.cell[0] as f32 + 0.5) * VOXEL_SIZE_M,
                prop.cell[1] as f32 * VOXEL_SIZE_M,
                (prop.cell[2] as f32 + 0.5) * VOXEL_SIZE_M,
            ],
            rotation: prop.rotation,
        })
        .collect();
    Blueprint::new(metadata, volume)
}

fn room_bounds_payload(bounds: &RoomBounds, floor: u32) -> ([f32; 4], [f32; 3]) {
    let bounds_m = [
        bounds.min_x as f32 * VOXEL_SIZE_M,
        bounds.min_z as f32 * VOXEL_SIZE_M,
        (bounds.max_x + 1) as f32 * VOXEL_SIZE_M,
        (bounds.max_z + 1) as f32 * VOXEL_SIZE_M,
    ];
    let band_height_m = bounds.band_height_voxels as f32 * VOXEL_SIZE_M;
    let center_m = [
        (bounds.min_x + bounds.max_x + 1) as f32 * 0.5 * VOXEL_SIZE_M,
        floor as f32 * FLOOR_BAND_M + band_height_m * 0.5,
        (bounds.min_z + bounds.max_z + 1) as f32 * 0.5 * VOXEL_SIZE_M,
    ];
    (bounds_m, center_m)
}

fn room_stats_to_dictionary(stats: &RoomStats) -> VarDictionary {
    debug_assert_eq!(stats.rooms.len(), stats.room_bounds.len());
    let mut result = VarDictionary::new();
    let mut rooms: Array<VarDictionary> = Array::new();
    for (room, bounds) in stats.rooms.iter().zip(&stats.room_bounds) {
        let mut item = VarDictionary::new();
        item.set("id", room.id.clone());
        item.set("type", room.room_type.clone());
        item.set("floor", room.floor as i64);
        item.set("area_m2", room.area_m2);
        item.set("volume_m3", room.volume_m3);
        item.set("beds", room.beds as i64);
        item.set("desks", room.desks as i64);
        item.set("doors", room.doors as i64);
        item.set("windows", room.windows as i64);
        let (bounds_m, center_m) = room_bounds_payload(bounds, room.floor);
        item.set(
            "bounds_m",
            &PackedFloat32Array::from(bounds_m.to_vec()).to_variant(),
        );
        item.set(
            "center_m",
            &PackedFloat32Array::from(center_m.to_vec()).to_variant(),
        );
        rooms.push(&item);
    }
    result.set("rooms", &rooms.to_variant());
    result.set("resident_capacity", stats.resident_capacity as i64);
    result.set("job_slots", stats.job_slots as i64);
    result.set("class", stats.class.clone());
    result.set("solid_voxels", stats.solid_voxels as i64);
    result.set("material_cost", slice_material_cost(&stats.materials));
    let mut materials = VarDictionary::new();
    for (key, value) in &stats.materials {
        materials.set(key.clone(), *value as i64);
    }
    result.set("materials", &materials.to_variant());
    result
}

struct TownsmithExtension;

#[gdextension]
unsafe impl ExtensionLibrary for TownsmithExtension {}

struct TerrainJob {
    id: u64,
    seed: u64,
    chunk_x: i64,
    chunk_z: i64,
    step: usize,
    deltas: Arc<RwLock<DeltaStore>>,
}

struct TerrainResult {
    id: u64,
    chunk_x: i64,
    chunk_z: i64,
    step: usize,
    mesh: Option<TerrainMesh>,
    error: Option<String>,
}

enum PollOutcome {
    Empty,
    Result(TerrainResult),
    Disconnected,
}

/// Fixed-size Rust worker pool for terrain generation and meshing.
///
/// Workers only touch Rust data. The main thread converts finished
/// [`TerrainMesh`] values into Godot resources in [`TownsmithCore::poll_terrain_chunk`].
struct TerrainBridge {
    jobs: Option<Sender<TerrainJob>>,
    results: Receiver<TerrainResult>,
    workers: Vec<JoinHandle<()>>,
    next_id: i64,
}

impl TerrainBridge {
    fn new() -> Self {
        let (job_tx, job_rx) = mpsc::channel::<TerrainJob>();
        let (result_tx, result_rx) = mpsc::channel::<TerrainResult>();
        let job_rx = Arc::new(Mutex::new(job_rx));

        let available = thread::available_parallelism().map_or(1, std::num::NonZero::get);
        let worker_count = available.saturating_sub(1).clamp(1, MAX_TERRAIN_WORKERS);
        let mut workers = Vec::with_capacity(worker_count);

        for index in 0..worker_count {
            let jobs = Arc::clone(&job_rx);
            let results = result_tx.clone();
            match thread::Builder::new()
                .name(format!("townsmith-terrain-{index}"))
                .spawn(move || worker_loop(jobs, results))
            {
                Ok(handle) => workers.push(handle),
                Err(error) => eprintln!("Townsmith: failed to spawn terrain worker: {error}"),
            }
        }

        Self {
            jobs: Some(job_tx),
            results: result_rx,
            workers,
            next_id: 1,
        }
    }

    fn request(
        &mut self,
        seed: u64,
        chunk_x: i64,
        chunk_z: i64,
        step: usize,
        deltas: Arc<RwLock<DeltaStore>>,
    ) -> i64 {
        let id = self.next_id;
        self.next_id = if id == i64::MAX { 1 } else { id + 1 };

        let job = TerrainJob {
            id: id as u64,
            seed,
            chunk_x,
            chunk_z,
            step,
            deltas,
        };
        let sent = self
            .jobs
            .as_ref()
            .is_some_and(|jobs| jobs.send(job).is_ok());
        if !sent {
            self.jobs = None;
            return -1;
        }
        id
    }

    fn poll(&self) -> PollOutcome {
        match self.results.try_recv() {
            Ok(result) => PollOutcome::Result(result),
            Err(TryRecvError::Empty) => PollOutcome::Empty,
            Err(TryRecvError::Disconnected) => PollOutcome::Disconnected,
        }
    }
}

impl Drop for TerrainBridge {
    fn drop(&mut self) {
        // Closing the job channel makes idle workers exit; joining prevents the
        // shared library from being unloaded while a worker is still running.
        self.jobs.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn worker_loop(jobs: Arc<Mutex<Receiver<TerrainJob>>>, results: Sender<TerrainResult>) {
    loop {
        let job = {
            let receiver = jobs.lock().unwrap_or_else(|error| error.into_inner());
            receiver.recv()
        };
        let Ok(job) = job else {
            break;
        };

        let deltas = Arc::clone(&job.deltas);
        let outcome = catch_unwind(AssertUnwindSafe(move || {
            let generator = WorldGenerator::new(job.seed);
            generator
                .generate(WorldChunkCoord::new(job.chunk_x, job.chunk_z))
                .map(|mut chunk| {
                    let deltas = deltas.read().unwrap_or_else(|error| error.into_inner());
                    deltas.apply_to(&mut chunk);
                    terrain_mesh_lod_with_sampler_and_skirt(
                        &chunk,
                        job.step,
                        |x, z| deltas.height_at(&generator, TerrainPos::new(x, z)),
                        DEFAULT_SKIRT_DEPTH_M,
                    )
                })
        }));

        let (mesh, error) = match outcome {
            Ok(Ok(mesh)) => (Some(mesh), None),
            Ok(Err(error)) => (None, Some(error.to_string())),
            Err(_) => (None, Some("terrain worker panicked".to_string())),
        };

        if results
            .send(TerrainResult {
                id: job.id,
                chunk_x: job.chunk_x,
                chunk_z: job.chunk_z,
                step: job.step,
                mesh,
                error,
            })
            .is_err()
        {
            break;
        }
    }
}

/// One editor volume plus the stats of its most recent mesh build.
///
/// The volume itself is single-threaded in practice; the `Mutex` in
/// [`TownsmithCore`] keeps the Godot class `Sync` and makes accidental
/// cross-thread access safe rather than undefined.
#[derive(Debug, Clone)]
struct EditorProp {
    kind: PropKind,
    cell: [u32; 3],
    rotation: f32,
}

struct EditorState {
    volume: VoxelVolume,
    props: Vec<EditorProp>,
    mesh_builds: u64,
    mesh_dirty: bool,
    last_mesh_quads: usize,
    last_mesh_vertices: usize,
    last_mesh_indices: usize,
    mesh_ms: f64,
}

impl EditorState {
    fn new(volume: VoxelVolume) -> Self {
        Self {
            volume,
            props: Vec::new(),
            mesh_builds: 0,
            // A newly created volume has no mesh yet, so consumers that act on
            // the dirty flag must build the initial surface.
            mesh_dirty: true,
            last_mesh_quads: 0,
            last_mesh_vertices: 0,
            last_mesh_indices: 0,
            mesh_ms: 0.0,
        }
    }

    /// Mark the volume as edited and forget the stats of the now-stale mesh.
    fn mark_dirty(&mut self) {
        self.mesh_dirty = true;
        self.last_mesh_quads = 0;
        self.last_mesh_vertices = 0;
        self.last_mesh_indices = 0;
        self.mesh_ms = 0.0;
    }
}

/// Coarse bridge object used by the Godot shell.
#[derive(GodotClass)]
#[class(init, base=RefCounted)]
struct TownsmithCore {
    terrain: Mutex<Option<TerrainBridge>>,
    editor: Mutex<Option<EditorState>>,
    world: Mutex<Option<PlacementStore>>,
    construction: Mutex<ConstructionStore>,
    roads: Mutex<Option<RoadStore>>,
    lots: Mutex<Option<LotStore>>,
    settlements: Mutex<Option<SettlementStore>>,
    population: Mutex<Option<CityRuntime>>,
    economy: Mutex<Option<Economy>>,
    industry: Mutex<Option<IndustryStore>>,
    terrain_deltas: Arc<RwLock<DeltaStore>>,
    terrain_seed: Mutex<Option<u64>>,
}

impl Default for TownsmithCore {
    fn default() -> Self {
        Self {
            terrain: Mutex::new(None),
            editor: Mutex::new(None),
            world: Mutex::new(None),
            construction: Mutex::new(ConstructionStore::default()),
            roads: Mutex::new(None),
            lots: Mutex::new(None),
            settlements: Mutex::new(None),
            population: Mutex::new(None),
            economy: Mutex::new(None),
            industry: Mutex::new(None),
            terrain_deltas: Arc::new(RwLock::new(DeltaStore::new())),
            terrain_seed: Mutex::new(None),
        }
    }
}

#[godot_api]
impl TownsmithCore {
    /// Core version string, useful for logs.
    #[func]
    fn version(&self) -> GString {
        GString::from(env!("CARGO_PKG_VERSION"))
    }

    /// Generate a synthetic city layout for the rendering benchmark.
    ///
    /// `tier` is `0` = Early, `1` = Mid, `2` = Late. Returns a dictionary:
    ///
    /// - `tier` (String), `seed` (int), `population` (int), `buildings` (int count)
    /// - `bounds` (`PackedFloat32Array`, `[min_x, min_z, max_x, max_z]`)
    /// - `positions` / `sizes` (`PackedVector3Array`, box centers / full extents)
    /// - `colors` (`PackedColorArray`, linear RGB)
    /// - `kinds` (`PackedInt32Array`, 0=house, 1=apartment, 2=shop, 3=office, 4=industry)
    /// - `checksum` (String, 64-bit layout checksum)
    ///
    /// On failure, returns `{ "error": "<message>" }`. Must be called on the
    /// Godot main thread because it creates Variants and packed arrays.
    #[func]
    fn build_city_layout(&self, tier: i64, seed: i64) -> VarDictionary {
        let Some(tier) = city_tier_from_code(tier) else {
            let mut result = VarDictionary::new();
            let message = format!("invalid_city_tier: {tier}");
            result.set("error", &GString::from(&message).to_variant());
            return result;
        };
        match CityLayout::generate(tier, seed as u64) {
            Ok(layout) => city_layout_to_dictionary(&layout),
            Err(error) => {
                let message = error.to_string();
                let mut result = VarDictionary::new();
                result.set("error", &GString::from(&message).to_variant());
                result
            }
        }
    }

    /// Build the deterministic Phase 0 hut and return it as a Godot mesh.
    #[func]
    fn build_demo_mesh(&self) -> Gd<ArrayMesh> {
        let Ok(chunk) = demo::hut() else {
            return ArrayMesh::new_gd();
        };
        let buffers = greedy_mesh(&chunk);

        let mut surface = SurfaceTool::new_gd();
        surface.begin(PrimitiveType::TRIANGLES);
        for triangle in buffers.indices.chunks(3) {
            for &index in triangle {
                let Some(vertex) = buffers.vertices.get(index as usize) else {
                    continue;
                };
                surface.set_normal(Vector3::new(
                    vertex.normal[0],
                    vertex.normal[1],
                    vertex.normal[2],
                ));
                surface.set_uv(Vector2::new(vertex.uv[0], vertex.uv[1]));
                surface.add_vertex(Vector3::new(
                    vertex.position[0],
                    vertex.position[1],
                    vertex.position[2],
                ));
            }
        }

        surface.commit().unwrap_or_else(ArrayMesh::new_gd)
    }

    /// Build the deterministic vertical-slice two-story house as a Godot mesh.
    ///
    /// The Rust fixture ([`demo::two_story_house`]) fills the default
    /// 5 × 3 × 5 chunk volume (16 m × 9.6 m × 16 m) and is shared with the
    /// `townsmith-bench` volume benchmark. The result is one packed-array
    /// surface with positions, normals, meter-scaled UVs, and indices. Returns
    /// an empty `ArrayMesh` if the fixture cannot be built.
    ///
    /// **Synchronous and main-thread only.** Whole-volume greedy meshing is
    /// ~75 ms on a desktop CPU; call this once at load, never per frame.
    #[func]
    fn build_slice_house_mesh(&self) -> Gd<ArrayMesh> {
        let Ok(volume) = demo::two_story_house() else {
            eprintln!("Townsmith: could not build the slice house fixture");
            return ArrayMesh::new_gd();
        };
        match voxel_mesh_to_godot(&volume.mesh()) {
            Ok(mesh) => mesh,
            Err(message) => {
                eprintln!("Townsmith: {message}");
                ArrayMesh::new_gd()
            }
        }
    }

    /// Counts for the deterministic vertical-slice house fixture.
    ///
    /// Returns `solid_voxels`, `quads`, `vertices`, `indices`, `buffer_bytes`
    /// (position/normal/UV vertex payload plus the index payload, in bytes),
    /// and `checksum` (16 lowercase hex digits). Godot surface counts are not
    /// reported here; query the returned `ArrayMesh` instead.
    ///
    /// **Synchronous and main-thread only.** This remeshes the fixture, so use
    /// it once at load for the benchmark report.
    #[func]
    fn slice_house_stats(&self) -> VarDictionary {
        let mut result = VarDictionary::new();
        let Ok(volume) = demo::two_story_house() else {
            result.set("error", "slice_house_build_failed");
            return result;
        };
        let buffers = volume.mesh();
        let vertex_bytes = buffers.vertices.len() * (3 * 4 + 3 * 4 + 2 * 4);
        let index_bytes = buffers.indices.len() * 4;
        let checksum = buffers.checksum();

        result.set("solid_voxels", i64::from(volume.solid_count()));
        result.set("quads", buffers.quad_count() as i64);
        result.set("vertices", buffers.vertices.len() as i64);
        result.set("indices", buffers.indices.len() as i64);
        result.set("buffer_bytes", (vertex_bytes + index_bytes) as i64);
        result.set(
            "checksum",
            &GString::from(&format!("{checksum:016x}")).to_variant(),
        );
        result
    }

    fn deltas_for_seed(&self, seed: u64) -> Arc<RwLock<DeltaStore>> {
        let active = *self
            .terrain_seed
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if active.is_some_and(|active| active != seed) {
            Arc::new(RwLock::new(DeltaStore::new()))
        } else {
            Arc::clone(&self.terrain_deltas)
        }
    }

    fn terrain_height_for_seed(&self, seed: u64, pos: TerrainPos) -> u16 {
        let active = *self
            .terrain_seed
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let generator = WorldGenerator::new(seed);
        if active == Some(seed) {
            let deltas = self
                .terrain_deltas
                .read()
                .unwrap_or_else(|error| error.into_inner());
            deltas.height_at(&generator, pos)
        } else {
            generator.height_at(pos)
        }
    }

    /// Generate one 256 m terrain chunk synchronously.
    ///
    /// Kept for tooling and tests. The streaming shell should use
    /// [`Self::request_terrain_chunk`] instead.
    #[func]
    fn build_terrain_mesh(&self, seed: i64, chunk_x: i64, chunk_z: i64) -> Gd<ArrayMesh> {
        self.build_terrain_mesh_lod(seed, chunk_x, chunk_z, 1)
    }

    /// Generate one terrain chunk synchronously at a mesh LOD stride.
    ///
    /// Kept for tooling and tests. Values outside `1..=256` are clamped so bad
    /// GDScript input cannot disable meshing.
    #[func]
    fn build_terrain_mesh_lod(
        &self,
        seed: i64,
        chunk_x: i64,
        chunk_z: i64,
        step: i64,
    ) -> Gd<ArrayMesh> {
        let step = clamp_step(step);
        let seed = seed as u64;
        let generator = WorldGenerator::new(seed);
        let Ok(mut chunk) = generator.generate(WorldChunkCoord::new(chunk_x, chunk_z)) else {
            return ArrayMesh::new_gd();
        };
        let deltas = self.deltas_for_seed(seed);
        let deltas = deltas.read().unwrap_or_else(|error| error.into_inner());
        deltas.apply_to(&mut chunk);
        let buffers = terrain_mesh_lod_with_sampler_and_skirt(
            &chunk,
            step,
            |x, z| deltas.height_at(&generator, TerrainPos::new(x, z)),
            DEFAULT_SKIRT_DEPTH_M,
        );
        terrain_mesh_to_godot(&buffers)
    }

    /// Queue a terrain chunk for worker-thread generation.
    ///
    /// Returns a request id, or `-1` if the worker pool is unavailable.
    /// Results are collected with [`Self::poll_terrain_chunk`] on the main
    /// thread. A result dictionary contains `id`, `chunk_x`, `chunk_z`, `step`,
    /// `mesh`, and an optional `error`.
    #[func]
    fn request_terrain_chunk(&self, seed: i64, chunk_x: i64, chunk_z: i64, step: i64) -> i64 {
        let step = clamp_step(step);
        let mut guard = self
            .terrain
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let bridge = guard.get_or_insert_with(TerrainBridge::new);
        let seed = seed as u64;
        bridge.request(seed, chunk_x, chunk_z, step, self.deltas_for_seed(seed))
    }

    /// Drop the terrain worker pool so the next request recreates it.
    ///
    /// Keeps terrain deltas and all other bridge state. Used after a worker
    /// disconnect instead of constructing a new `TownsmithCore`.
    #[func]
    fn reset_terrain_workers(&self) -> bool {
        let mut guard = self
            .terrain
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        guard.take();
        true
    }

    /// Poll one finished worker-thread terrain chunk.
    ///
    /// Returns an empty `Dictionary` when no result is ready. If the worker
    /// pool has disconnected, returns `{"id": -1, "error":
    /// "worker_pool_disconnected"}`. The caller should poll in a loop until it
    /// receives an empty dictionary.
    #[func]
    fn poll_terrain_chunk(&self) -> VarDictionary {
        let result = {
            let guard = self
                .terrain
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let Some(bridge) = guard.as_ref() else {
                return VarDictionary::new();
            };
            match bridge.poll() {
                PollOutcome::Empty => return VarDictionary::new(),
                PollOutcome::Disconnected => {
                    let mut output = VarDictionary::new();
                    output.set("id", -1);
                    output.set("error", "worker_pool_disconnected");
                    return output;
                }
                PollOutcome::Result(result) => result,
            }
        };

        let mesh = result
            .mesh
            .as_ref()
            .map_or_else(ArrayMesh::new_gd, terrain_mesh_to_godot);

        let mut output = VarDictionary::new();
        output.set("id", result.id as i64);
        output.set("chunk_x", result.chunk_x);
        output.set("chunk_z", result.chunk_z);
        output.set("step", result.step as i64);
        output.set("mesh", &mesh);
        if let Some(error) = result.error {
            output.set("error", error);
        }
        output
    }

    /// Validate a blueprint placement without mutating the world.
    ///
    /// Returns `{ ok, center, foundation_m, footprint, rotation_deg,
    /// blueprint_hash, bounds }` on success and `{ ok: false, error,
    /// error_code }` on failure.
    #[func]
    fn world_validate_placement(
        &self,
        seed: i64,
        path: GString,
        x: f64,
        z: f64,
        rotation: i64,
    ) -> VarDictionary {
        let (seed, request) = match world_placement_request(seed, &path.to_string(), x, z, rotation)
        {
            Ok(value) => value,
            Err(error) => return error,
        };
        let height_at =
            |cell_x, cell_z| self.terrain_height_for_seed(seed, TerrainPos::new(cell_x, cell_z));
        let guard = self.world.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(store) = guard.as_ref() {
            if store.seed != seed {
                return placement_simple_error(
                    "seed_mismatch",
                    "the placement store belongs to a different seed",
                );
            }
        }
        let result = match guard.as_ref() {
            Some(store) => store.validate(&request, height_at),
            None => PlacementStore::new(seed).validate(&request, height_at),
        };
        match result {
            Ok(plan) => {
                let mut result = VarDictionary::new();
                result.set("ok", true);
                result.set(
                    "center",
                    &PackedFloat32Array::from(plan.center.to_vec()).to_variant(),
                );
                result.set("foundation_m", f64::from(plan.foundation_m));
                result.set(
                    "footprint",
                    &PackedFloat32Array::from(plan.footprint_m.to_vec()).to_variant(),
                );
                result.set("rotation_deg", i64::from(plan.rotation_deg));
                result.set(
                    "blueprint_hash",
                    &GString::from(request.blueprint_hash.as_str()),
                );
                result.set(
                    "bounds",
                    &PackedFloat32Array::from(request.bounds_m.to_vec()).to_variant(),
                );
                result
            }
            Err(error) => placement_error_result(&error),
        }
    }

    /// Return metadata for a saved blueprint without touching the world.
    ///
    /// The scene uses this to size the placement ghost and show the blueprint
    /// name, cost, and room count before the first placement.
    #[func]
    fn world_blueprint_info(&self, path: GString) -> VarDictionary {
        let path = match blueprint_path_from_arg(&path.to_string()) {
            Ok(path) => path,
            Err(error) => return placement_simple_error("invalid_blueprint_path", error),
        };
        let blueprint = match load_tsbp(&path) {
            Ok(blueprint) => blueprint,
            Err(error) => {
                return placement_simple_error(
                    blueprint_error_code(&error),
                    format!("{}: {error}", path.display()),
                )
            }
        };
        let (materials, _) = count_materials(&blueprint.volume);
        let computed_cost = slice_material_cost(&materials);
        let stored_cost = blueprint.metadata.stats.material_cost;
        let material_cost = if stored_cost.is_finite() && stored_cost > 0.0 {
            stored_cost
        } else {
            computed_cost
        };
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("id", &GString::from(blueprint.metadata.id.as_str()));
        result.set("name", &GString::from(blueprint.metadata.name.as_str()));
        result.set(
            "hash",
            &GString::from(blueprint.metadata.content_hash.as_str()),
        );
        result.set(
            "bounds",
            &PackedFloat32Array::from(blueprint.metadata.bounds_m.to_vec()).to_variant(),
        );
        result.set("solid_voxels", blueprint.metadata.solid_voxels as i64);
        result.set("rooms", blueprint.metadata.rooms.len() as i64);
        result.set(
            "resident_capacity",
            i64::from(blueprint.metadata.stats.resident_capacity),
        );
        result.set("job_slots", i64::from(blueprint.metadata.stats.job_slots));
        result.set("material_cost", material_cost);
        result
    }

    /// Place a `.tsbp` blueprint in the world after validation.
    ///
    /// Returns `{ ok, seed, count, checksum, placement }` on success.
    #[func]
    fn world_place_blueprint(
        &self,
        seed: i64,
        path: GString,
        x: f64,
        z: f64,
        rotation: i64,
    ) -> VarDictionary {
        let (seed, request) = match world_placement_request(seed, &path.to_string(), x, z, rotation)
        {
            Ok(value) => value,
            Err(error) => return error,
        };
        let height_at =
            |cell_x, cell_z| self.terrain_height_for_seed(seed, TerrainPos::new(cell_x, cell_z));
        let mut guard = self.world.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(store) = guard.as_ref() {
            if store.seed != seed {
                return placement_simple_error(
                    "seed_mismatch",
                    "the placement store belongs to a different seed",
                );
            }
        }
        let created = guard.is_none();
        let placed = {
            let store = guard.get_or_insert_with(|| PlacementStore::new(seed));
            store.place(&request, height_at)
        };
        if placed.is_err() && created {
            // A rejected first placement must not leave an empty store with
            // this seed behind; otherwise a later attempt with another seed
            // would fail with `seed_mismatch`.
            *guard = None;
        }
        match placed {
            Ok(placement) => {
                let (count, checksum) = {
                    let store = guard.as_ref().expect("a successful place keeps a store");
                    (store.placements.len(), store.checksum())
                };
                let construction_result = self
                    .construction
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .add_site(placement.id, request.solid_voxels)
                    .map(|_| ());
                if let Err(error) = construction_result {
                    if let Some(store) = guard.as_mut() {
                        store.remove(placement.id);
                    }
                    if created {
                        *guard = None;
                    }
                    return placement_simple_error("construction_failed", error.to_string());
                }
                let mut result = VarDictionary::new();
                result.set("ok", true);
                result.set("seed", seed as i64);
                result.set("count", count as i64);
                result.set("checksum", &GString::from(&format!("{checksum:016x}")));
                result.set(
                    "placement",
                    &placement_to_dictionary(&placement).to_variant(),
                );
                result
            }
            Err(error) => placement_error_result(&error),
        }
    }

    /// Return the active placement store.
    #[func]
    fn world_placements(&self) -> VarDictionary {
        let guard = self.world.lock().unwrap_or_else(|error| error.into_inner());
        let mut result = VarDictionary::new();
        result.set("ok", true);
        match guard.as_ref() {
            Some(store) => {
                result.set("open", true);
                let mut placements: Array<VarDictionary> = Array::new();
                for placement in &store.placements {
                    placements.push(&placement_to_dictionary(placement));
                }
                result.set("seed", store.seed as i64);
                result.set("next_id", store.next_id as i64);
                result.set("count", store.placements.len() as i64);
                result.set(
                    "checksum",
                    &GString::from(&format!("{:016x}", store.checksum())),
                );
                result.set("placements", &placements.to_variant());
            }
            None => {
                result.set("open", false);
                result.set("seed", 0);
                result.set("next_id", 1);
                result.set("count", 0);
                result.set("checksum", &GString::from("0000000000000000"));
                result.set("placements", &Array::<VarDictionary>::new().to_variant());
            }
        }
        result
    }

    /// Number of placements in the active world.
    #[func]
    fn world_placement_count(&self) -> i64 {
        let guard = self.world.lock().unwrap_or_else(|error| error.into_inner());
        guard
            .as_ref()
            .map_or(0, |store| store.placements.len() as i64)
    }

    /// Stable checksum of the active placement store.
    #[func]
    fn world_placement_checksum(&self) -> GString {
        let guard = self.world.lock().unwrap_or_else(|error| error.into_inner());
        let checksum = guard.as_ref().map_or(0, PlacementStore::checksum);
        GString::from(&format!("{checksum:016x}"))
    }

    /// Validate a straight road without mutating the world.
    #[func]
    fn road_validate(
        &self,
        seed: i64,
        x1: i64,
        z1: i64,
        x2: i64,
        z2: i64,
        width: i64,
    ) -> VarDictionary {
        let seed = match u64::try_from(seed) {
            Ok(seed) => seed,
            Err(_) => {
                return placement_simple_error("invalid_seed", "road seed must be non-negative")
            }
        };
        let request = match road_request_from_args(x1, z1, x2, z2, width) {
            Ok(request) => request,
            Err(error) => return error,
        };
        {
            let world = self.world.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(store) = world.as_ref() {
                if store.seed != seed {
                    return placement_simple_error(
                        "seed_mismatch",
                        "the world belongs to a different seed",
                    );
                }
            }
        }
        let roads = self.roads.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(store) = roads.as_ref() {
            if store.seed != seed {
                return placement_simple_error(
                    "seed_mismatch",
                    "the road store belongs to a different seed",
                );
            }
        }
        let result = match roads.as_ref() {
            Some(store) => store.validate(&request, |x, z| {
                self.terrain_height_for_seed(seed, TerrainPos::new(x, z))
            }),
            None => RoadStore::new(seed).validate(&request, |x, z| {
                self.terrain_height_for_seed(seed, TerrainPos::new(x, z))
            }),
        };
        match result {
            Ok(()) => {
                let mut result = VarDictionary::new();
                result.set("ok", true);
                result.set("length_m", road_length(&request));
                result.set("width_m", i64::from(request.width_m));
                result
            }
            Err(error) => road_error_result(&error),
        }
    }

    /// Add one straight road after terrain validation.
    #[func]
    fn road_add(&self, seed: i64, x1: i64, z1: i64, x2: i64, z2: i64, width: i64) -> VarDictionary {
        let seed = match u64::try_from(seed) {
            Ok(seed) => seed,
            Err(_) => {
                return placement_simple_error("invalid_seed", "road seed must be non-negative")
            }
        };
        let request = match road_request_from_args(x1, z1, x2, z2, width) {
            Ok(request) => request,
            Err(error) => return error,
        };
        let mut world = self.world.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(store) = world.as_ref() {
            if store.seed != seed {
                return placement_simple_error(
                    "seed_mismatch",
                    "the world belongs to a different seed",
                );
            }
        }
        let created_world = world.is_none();
        let mut roads = self.roads.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(store) = roads.as_ref() {
            if store.seed != seed {
                return placement_simple_error(
                    "seed_mismatch",
                    "the road store belongs to a different seed",
                );
            }
        }
        let created_roads = roads.is_none();
        let result = {
            let store = roads.get_or_insert_with(|| RoadStore::new(seed));
            store.add(&request, |x, z| {
                self.terrain_height_for_seed(seed, TerrainPos::new(x, z))
            })
        };
        let road = match result {
            Ok(road) => road,
            Err(error) => {
                if created_roads {
                    *roads = None;
                }
                if created_world {
                    *world = None;
                }
                return road_error_result(&error);
            }
        };
        if created_world {
            *world = Some(PlacementStore::new(seed));
        }
        let store = roads.as_ref().expect("a successful add keeps a road store");
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("seed", seed as i64);
        result.set("count", store.roads.len() as i64);
        result.set("road", &road_to_dictionary(&road).to_variant());
        result
    }

    /// Current road network.
    #[func]
    fn roads(&self) -> VarDictionary {
        let store = self.roads.lock().unwrap_or_else(|error| error.into_inner());
        let mut result = VarDictionary::new();
        result.set("ok", true);
        match store.as_ref() {
            Some(store) => {
                let mut roads: Array<VarDictionary> = Array::new();
                for road in &store.roads {
                    roads.push(&road_to_dictionary(road));
                }
                result.set("open", true);
                result.set("seed", store.seed as i64);
                result.set("next_id", store.next_id as i64);
                result.set("count", store.roads.len() as i64);
                result.set(
                    "checksum",
                    &GString::from(&format!("{:016x}", store.checksum())),
                );
                result.set("roads", &roads.to_variant());
            }
            None => {
                result.set("open", false);
                result.set("seed", 0);
                result.set("next_id", 1);
                result.set("count", 0);
                result.set("checksum", &GString::from("0000000000000000"));
                result.set("roads", &Array::<VarDictionary>::new().to_variant());
            }
        }
        result
    }

    /// Number of road segments.
    #[func]
    fn road_count(&self) -> i64 {
        self.roads
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .map_or(0, |store| store.roads.len() as i64)
    }

    /// Stable road checksum.
    #[func]
    fn road_checksum(&self) -> GString {
        let checksum = self
            .roads
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .map_or(0, RoadStore::checksum);
        GString::from(&format!("{checksum:016x}"))
    }

    /// Remove every road segment.
    #[func]
    fn road_clear(&self) -> bool {
        if let Some(store) = self
            .roads
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_mut()
        {
            store.clear();
        }
        true
    }

    /// Remove one road by id.
    #[func]
    fn road_remove(&self, id: i64) -> bool {
        let Ok(id) = u64::try_from(id) else {
            return false;
        };
        self.roads
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_mut()
            .is_some_and(|store| store.remove(id))
    }

    /// Validate a zoned lot without mutating the store.
    #[func]
    fn lot_validate(
        &self,
        seed: i64,
        min_x: i64,
        min_z: i64,
        max_x: i64,
        max_z: i64,
        zone: GString,
    ) -> VarDictionary {
        let (seed, request) = match lot_request_from_args(seed, min_x, min_z, max_x, max_z, &zone) {
            Ok(value) => value,
            Err(error) => return error,
        };
        let world = self.world.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(store) = world.as_ref() {
            if store.seed != seed {
                return placement_simple_error(
                    "seed_mismatch",
                    "the world belongs to a different seed",
                );
            }
        }
        let roads = self.roads.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(store) = roads.as_ref() {
            if store.seed != seed {
                return placement_simple_error(
                    "seed_mismatch",
                    "the road store belongs to a different seed",
                );
            }
        }
        let empty_roads = RoadStore::new(seed);
        let road_store = roads.as_ref().unwrap_or(&empty_roads);
        let lots = self.lots.lock().unwrap_or_else(|error| error.into_inner());
        let lot_store = lots
            .as_ref()
            .cloned()
            .unwrap_or_else(|| LotStore::new(seed));
        match lot_store.validate(
            &request,
            |x, z| self.terrain_height_for_seed(seed, TerrainPos::new(x, z)),
            road_store,
        ) {
            Ok(()) => lot_validation_result(&request),
            Err(error) => lot_error_result(&error),
        }
    }

    /// Add a zoned lot after validation. A first successful lot opens the
    /// world store for the seed.
    #[func]
    fn lot_add(
        &self,
        seed: i64,
        min_x: i64,
        min_z: i64,
        max_x: i64,
        max_z: i64,
        zone: GString,
    ) -> VarDictionary {
        let (seed, request) = match lot_request_from_args(seed, min_x, min_z, max_x, max_z, &zone) {
            Ok(value) => value,
            Err(error) => return error,
        };
        let mut world = self.world.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(store) = world.as_ref() {
            if store.seed != seed {
                return placement_simple_error(
                    "seed_mismatch",
                    "the world belongs to a different seed",
                );
            }
        }
        let roads = self.roads.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(store) = roads.as_ref() {
            if store.seed != seed {
                return placement_simple_error(
                    "seed_mismatch",
                    "the road store belongs to a different seed",
                );
            }
        }
        let empty_roads = RoadStore::new(seed);
        let road_store = roads.as_ref().unwrap_or(&empty_roads);
        let created_world = world.is_none();
        let mut lots = self.lots.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(store) = lots.as_ref() {
            if store.seed != seed {
                return placement_simple_error(
                    "seed_mismatch",
                    "the lot store belongs to a different seed",
                );
            }
        }
        let created_lots = lots.is_none();
        let result = {
            let lot_store = lots.get_or_insert_with(|| LotStore::new(seed));
            lot_store.add(
                &request,
                |x, z| self.terrain_height_for_seed(seed, TerrainPos::new(x, z)),
                road_store,
            )
        };
        match result {
            Ok(lot) => {
                if created_world {
                    *world = Some(PlacementStore::new(seed));
                }
                let lot_store = lots.as_ref().expect("a successful lot keeps a store");
                let mut result = VarDictionary::new();
                result.set("ok", true);
                result.set("seed", seed as i64);
                result.set("count", lot_store.lots.len() as i64);
                result.set(
                    "checksum",
                    &GString::from(&format!("{:016x}", lot_store.checksum())),
                );
                result.set("lot", &lot_to_dictionary(&lot).to_variant());
                result
            }
            Err(error) => {
                if created_lots {
                    *lots = None;
                }
                if created_world {
                    *world = None;
                }
                lot_error_result(&error)
            }
        }
    }

    /// Return the active lot store.
    #[func]
    fn lots(&self) -> VarDictionary {
        let store = self.lots.lock().unwrap_or_else(|error| error.into_inner());
        let mut result = VarDictionary::new();
        result.set("ok", true);
        match store.as_ref() {
            Some(store) => {
                let mut lots: Array<VarDictionary> = Array::new();
                for lot in &store.lots {
                    lots.push(&lot_to_dictionary(lot));
                }
                result.set("open", true);
                result.set("seed", store.seed as i64);
                result.set("next_id", store.next_id as i64);
                result.set("count", store.lots.len() as i64);
                result.set(
                    "checksum",
                    &GString::from(&format!("{:016x}", store.checksum())),
                );
                result.set("lots", &lots.to_variant());
            }
            None => {
                result.set("open", false);
                result.set("seed", 0);
                result.set("next_id", 1);
                result.set("count", 0);
                result.set("checksum", &GString::from("0000000000000000"));
                result.set("lots", &Array::<VarDictionary>::new().to_variant());
            }
        }
        result
    }

    /// Number of zoned lots.
    #[func]
    fn lot_count(&self) -> i64 {
        self.lots
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .map_or(0, |store| store.lots.len() as i64)
    }

    /// Stable checksum of the active lot store.
    #[func]
    fn lot_checksum(&self) -> GString {
        let store = self.lots.lock().unwrap_or_else(|error| error.into_inner());
        let checksum = store.as_ref().map_or(0, LotStore::checksum);
        GString::from(&format!("{checksum:016x}"))
    }

    /// Remove every lot.
    #[func]
    fn lot_clear(&self) -> bool {
        let mut store = self.lots.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(store) = store.as_mut() {
            store.clear();
        }
        true
    }

    /// Remove one lot by id.
    #[func]
    fn lot_remove(&self, id: i64) -> bool {
        let Ok(id) = u64::try_from(id) else {
            return false;
        };
        self.lots
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_mut()
            .is_some_and(|store| store.remove(id))
    }

    /// Found a settlement and open the world layer if needed.
    #[func]
    fn settlement_found(
        &self,
        seed: i64,
        name: GString,
        owner: GString,
        x: i64,
        z: i64,
        radius: i64,
    ) -> VarDictionary {
        let Ok(seed) = u64::try_from(seed) else {
            return placement_simple_error("invalid_seed", "world seed must be non-negative");
        };
        let Ok(x) = i32::try_from(x) else {
            return placement_simple_error(
                "coordinate_out_of_range",
                "settlement x must fit a 32-bit world coordinate",
            );
        };
        let Ok(z) = i32::try_from(z) else {
            return placement_simple_error(
                "coordinate_out_of_range",
                "settlement z must fit a 32-bit world coordinate",
            );
        };
        let Ok(radius) = u32::try_from(radius) else {
            return placement_simple_error("invalid_radius", "settlement radius must be positive");
        };
        let request = SettlementRequest {
            name: name.to_string(),
            owner: owner.to_string(),
            center: [x, z],
            radius_m: radius,
        };
        let mut world = self.world.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(store) = world.as_ref() {
            if store.seed != seed {
                return placement_simple_error(
                    "seed_mismatch",
                    "the world belongs to a different seed",
                );
            }
        }
        let roads = self.roads.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(store) = roads.as_ref() {
            if store.seed != seed {
                return placement_simple_error(
                    "seed_mismatch",
                    "the road store belongs to a different seed",
                );
            }
        }
        let lots = self.lots.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(store) = lots.as_ref() {
            if store.seed != seed {
                return placement_simple_error(
                    "seed_mismatch",
                    "the lot store belongs to a different seed",
                );
            }
        }
        let mut settlements = self
            .settlements
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(store) = settlements.as_ref() {
            if store.seed != seed {
                return placement_simple_error(
                    "seed_mismatch",
                    "the settlement layer belongs to a different seed",
                );
            }
        }
        let created_world = world.is_none();
        let created_settlements = settlements.is_none();
        let result = {
            let store = settlements.get_or_insert_with(|| SettlementStore::new(seed));
            store.found(&request)
        };
        match result {
            Ok(settlement) => {
                if created_world {
                    *world = Some(PlacementStore::new(seed));
                }
                let store = settlements
                    .as_ref()
                    .expect("a successful settlement keeps a store");
                let mut result = VarDictionary::new();
                result.set("ok", true);
                result.set("seed", seed as i64);
                result.set("count", store.count() as i64);
                result.set(
                    "checksum",
                    &GString::from(&format!("{:016x}", store.checksum())),
                );
                result.set(
                    "settlement",
                    &settlement_to_dictionary(&settlement).to_variant(),
                );
                result
            }
            Err(error) => {
                if created_settlements {
                    *settlements = None;
                }
                if created_world {
                    *world = None;
                }
                settlement_error_result(&error)
            }
        }
    }

    /// Return the active settlement layer.
    #[func]
    fn settlements(&self) -> VarDictionary {
        let store = self
            .settlements
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut result = VarDictionary::new();
        result.set("ok", true);
        match store.as_ref() {
            Some(store) => {
                let mut settlements: Array<VarDictionary> = Array::new();
                for settlement in store.settlements() {
                    settlements.push(&settlement_to_dictionary(settlement));
                }
                result.set("open", true);
                result.set("seed", store.seed as i64);
                result.set("next_id", store.next_id as i64);
                result.set("count", store.count() as i64);
                result.set(
                    "checksum",
                    &GString::from(&format!("{:016x}", store.checksum())),
                );
                result.set("settlements", &settlements.to_variant());
            }
            None => {
                result.set("open", false);
                result.set("seed", 0);
                result.set("next_id", 1);
                result.set("count", 0);
                result.set("checksum", &GString::from("0000000000000000"));
                result.set("settlements", &Array::<VarDictionary>::new().to_variant());
            }
        }
        result
    }

    /// Number of settlements.
    #[func]
    fn settlement_count(&self) -> i64 {
        self.settlements
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .map_or(0, |store| store.count() as i64)
    }

    /// Stable checksum of the settlement layer.
    #[func]
    fn settlement_checksum(&self) -> GString {
        let store = self
            .settlements
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let checksum = store.as_ref().map_or(0, SettlementStore::checksum);
        GString::from(&format!("{checksum:016x}"))
    }

    /// Remove every settlement; the seed and next id stay.
    #[func]
    fn settlement_clear(&self) -> bool {
        let mut store = self
            .settlements
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(store) = store.as_mut() {
            store.clear();
        }
        true
    }

    /// Remove one settlement by id.
    #[func]
    fn settlement_remove(&self, id: i64) -> bool {
        let Ok(id) = u64::try_from(id) else {
            return false;
        };
        self.settlements
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_mut()
            .is_some_and(|store| store.remove(id))
    }

    /// Owner of a world cell, or `{ ok, found: false }`.
    #[func]
    fn settlement_at(&self, x: i64, z: i64) -> VarDictionary {
        if x.unsigned_abs() > 1_000_000 || z.unsigned_abs() > 1_000_000 {
            return placement_simple_error(
                "coordinate_out_of_range",
                "settlement lookup coordinates must be within ±1,000,000 m",
            );
        }
        let (Ok(x), Ok(z)) = (i32::try_from(x), i32::try_from(z)) else {
            return placement_simple_error(
                "coordinate_out_of_range",
                "settlement lookup coordinates must fit 32-bit integers",
            );
        };
        let store = self
            .settlements
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut result = VarDictionary::new();
        result.set("ok", true);
        match store.as_ref().and_then(|store| store.owner_at(x, z)) {
            Some(settlement) => {
                result.set("found", true);
                result.set(
                    "settlement",
                    &settlement_to_dictionary(settlement).to_variant(),
                );
            }
            None => {
                result.set("found", false);
            }
        }
        result
    }

    /// Remove every placement. The seed and an empty store stay open.
    #[func]
    fn world_clear_placements(&self) -> bool {
        let mut guard = self.world.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(store) = guard.as_mut() {
            store.clear();
        }
        self.construction
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        self.population
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_mut()
            .map(CityRuntime::clear);
        self.industry
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_mut()
            .map(IndustryStore::clear);
        true
    }

    /// Remove one placement by id.
    #[func]
    fn world_remove_placement(&self, id: i64) -> bool {
        let Ok(id) = u64::try_from(id) else {
            return false;
        };
        let mut guard = self.world.lock().unwrap_or_else(|error| error.into_inner());
        let removed = guard.as_mut().is_some_and(|store| store.remove(id));
        if removed {
            self.construction
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove_site(id);
            if let Some(runtime) = self
                .population
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_mut()
            {
                runtime.remove(id);
            }
            if let Some(industry) = self
                .industry
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_mut()
            {
                industry.remove_site(id);
            }
        }
        removed
    }

    /// Advance every construction site by `delta_seconds`.
    ///
    /// Returns `{ ok, completed[], phase_changed[], active, stalled,
    /// checksum }`. The shell updates site visuals from
    /// [`Self::construction_sites`].
    #[func]
    fn construction_tick(&self, delta_seconds: f64) -> VarDictionary {
        if !delta_seconds.is_finite() {
            return placement_simple_error("invalid_delta", "construction delta must be finite");
        }
        let mut store = self
            .construction
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let report = store.tick(delta_seconds);
        let mut completed: Array<i64> = Array::new();
        for id in &report.completed {
            completed.push(*id as i64);
        }
        let mut phase_changed: Array<i64> = Array::new();
        for id in &report.phase_changed {
            phase_changed.push(*id as i64);
        }
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("completed", &completed.to_variant());
        result.set("phase_changed", &phase_changed.to_variant());
        result.set("active", report.active as i64);
        result.set("stalled", report.stalled as i64);
        result.set(
            "checksum",
            &GString::from(&format!("{:016x}", store.checksum())),
        );
        result
    }

    /// Current construction sites.
    #[func]
    fn construction_sites(&self) -> VarDictionary {
        let store = self
            .construction
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut sites: Array<VarDictionary> = Array::new();
        for site in store.sites.values() {
            sites.push(&construction_site_to_dictionary(site));
        }
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("count", sites.len() as i64);
        result.set("sites", &sites.to_variant());
        result.set(
            "checksum",
            &GString::from(&format!("{:016x}", store.checksum())),
        );
        result
    }

    /// Construction counts and shared resources.
    #[func]
    fn construction_summary(&self) -> VarDictionary {
        let store = self
            .construction
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let active = store
            .sites
            .values()
            .filter(|site| !site.is_complete())
            .count();
        let stalled = store
            .sites
            .values()
            .filter(|site| !site.is_complete() && site.stall_reason.is_some())
            .count();
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("sites", store.site_count() as i64);
        result.set("active", active as i64);
        result.set("stalled", stalled as i64);
        result.set("complete", (store.site_count() - active) as i64);
        result.set("crew_capacity", i64::from(store.crew_capacity));
        result.set("materials_stock", store.materials_stock as i64);
        result.set(
            "checksum",
            &GString::from(&format!("{:016x}", store.checksum())),
        );
        result
    }

    /// Derived city ledger from completed buildings.
    ///
    /// This is computed on demand; it is not a separate save field. Only
    /// completed construction sites contribute population or jobs.
    #[func]
    fn city_summary(&self) -> VarDictionary {
        // Hold the three stores in the established world -> construction ->
        // settlement order so the HUD cannot read a torn placement/site
        // snapshot. `summarize` only needs references, so no clone is needed.
        let placements = self.world.lock().unwrap_or_else(|error| error.into_inner());
        let construction = self
            .construction
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let settlements = self
            .settlements
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let settlement_count = settlements.as_ref().map_or(0, SettlementStore::count);
        let population = self
            .population
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let summary = match placements.as_ref() {
            Some(placements) => {
                let empty_runtime = CityRuntime::new(placements.seed);
                let runtime = population
                    .as_ref()
                    .filter(|runtime| runtime.seed == placements.seed)
                    .unwrap_or(&empty_runtime);
                summarize(placements, &construction, runtime, settlement_count)
            }
            None => CitySummary {
                settlements: settlement_count,
                ..CitySummary::default()
            },
        };
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("population", summary.population as i64);
        result.set("housing_capacity", summary.housing_capacity as i64);
        result.set("vacancies", summary.vacancies as i64);
        result.set("jobs", summary.jobs as i64);
        result.set("employed", summary.employed as i64);
        result.set("unemployed", summary.unemployed as i64);
        result.set("happiness", f64::from(summary.happiness));
        result.set("food", f64::from(summary.food));
        result.set("water", f64::from(summary.water));
        result.set("rest", f64::from(summary.rest));
        result.set("average_commute_m", f64::from(summary.average_commute_m));
        result.set("daily_wages", summary.daily_wages);
        result.set("completed_buildings", summary.completed_buildings as i64);
        result.set("construction_sites", summary.construction_sites as i64);
        result.set("settlements", summary.settlements as i64);
        result
    }

    /// Advance households by `seconds`: move-in, jobs, and needs.
    #[func]
    fn population_tick(&self, seed: i64, seconds: f64) -> VarDictionary {
        let Ok(seed) = u64::try_from(seed) else {
            return placement_simple_error("invalid_seed", "world seed must be non-negative");
        };
        if !seconds.is_finite()
            || !(0.0..=core_city::MAX_POPULATION_TICK_SECONDS).contains(&seconds)
        {
            return placement_simple_error("invalid_delta", "population delta is outside 0..=3600");
        }
        let placements = self.world.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(placements) = placements.as_ref() {
            if placements.seed != seed {
                return placement_simple_error(
                    "seed_mismatch",
                    "the placement store belongs to a different seed",
                );
            }
        }
        let construction = self
            .construction
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (homes, workplaces) = match placements.as_ref() {
            Some(placements) => (
                completed_homes(placements, &construction),
                completed_workplaces(placements, &construction),
            ),
            None => (Vec::new(), Vec::new()),
        };
        let mut population = self
            .population
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if population
            .as_ref()
            .is_some_and(|runtime| runtime.seed != seed)
        {
            return placement_simple_error(
                "seed_mismatch",
                "the household runtime belongs to a different seed",
            );
        }
        let runtime = population.get_or_insert_with(|| CityRuntime::new(seed));
        let food_supply = self
            .industry
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .filter(|industry| industry.seed == seed)
            .map_or(0.0, |industry| industry.food_supply);
        let report =
            runtime.tick_with_workplaces_and_supply(seconds, &homes, &workplaces, food_supply);
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("moved_in", i64::from(report.moved_in));
        result.set("population", report.population as i64);
        result.set("vacancies", report.vacancies as i64);
        result.set("workers", report.workers as i64);
        result.set("happiness", f64::from(runtime.happiness()));
        result.set("average_commute_m", f64::from(runtime.average_commute_m()));
        result.set("daily_wages", runtime.daily_wages());
        result.set(
            "checksum",
            &GString::from(&format!("{:016x}", runtime.checksum())),
        );
        result
    }

    /// Return household records and the move-in policy.
    #[func]
    fn population_state(&self) -> VarDictionary {
        let guard = self
            .population
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut result = VarDictionary::new();
        result.set("ok", true);
        match guard.as_ref() {
            Some(runtime) => {
                result.set("open", true);
                result.set("seed", runtime.seed as i64);
                result.set("move_in_per_second", f64::from(runtime.move_in_per_second));
                result.set("population", runtime.population() as i64);
                result.set("housing_capacity", runtime.housing_capacity() as i64);
                result.set("vacancies", runtime.vacancies() as i64);
                result.set("workers", runtime.workers() as i64);
                result.set("happiness", f64::from(runtime.happiness()));
                result.set("average_commute_m", f64::from(runtime.average_commute_m()));
                result.set("daily_wages", runtime.daily_wages());
                let mut households: Array<VarDictionary> = Array::new();
                for (&id, household) in &runtime.households {
                    let mut item = VarDictionary::new();
                    item.set("placement_id", id as i64);
                    item.set("capacity", i64::from(household.capacity));
                    item.set("residents", i64::from(household.residents));
                    item.set("workers", i64::from(household.workers));
                    item.set("food", f64::from(household.food));
                    item.set("water", f64::from(household.water));
                    item.set("rest", f64::from(household.rest));
                    item.set("commute_m", f64::from(household.commute_m));
                    item.set("income_per_day", household.income_per_day);
                    item.set("happiness", f64::from(household.happiness()));
                    households.push(&item);
                }
                result.set("households", &households.to_variant());
                result.set(
                    "checksum",
                    &GString::from(&format!("{:016x}", runtime.checksum())),
                );
            }
            None => {
                result.set("open", false);
                result.set("seed", 0);
                result.set("move_in_per_second", 0.0);
                result.set("population", 0);
                result.set("housing_capacity", 0);
                result.set("vacancies", 0);
                result.set("workers", 0);
                result.set("happiness", 0.0);
                result.set("households", &Array::<VarDictionary>::new().to_variant());
                result.set("checksum", &GString::from("0000000000000000"));
            }
        }
        result
    }

    /// Set the move-in rate in residents per simulated second.
    #[func]
    fn population_set_rate(&self, rate: f64) -> bool {
        if !rate.is_finite() {
            return false;
        }
        let mut guard = self
            .population
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(runtime) = guard.as_mut() else {
            return false;
        };
        runtime.set_move_in_per_second(rate as f32);
        true
    }

    /// Remove every household.
    #[func]
    fn population_clear(&self) -> bool {
        let mut guard = self
            .population
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(runtime) = guard.as_mut() {
            runtime.clear();
        }
        true
    }

    /// Household runtime checksum.
    #[func]
    fn population_checksum(&self) -> GString {
        let guard = self
            .population
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let checksum = guard.as_ref().map_or(0, CityRuntime::checksum);
        GString::from(&format!("{checksum:016x}"))
    }

    /// Return the treasury state.
    #[func]
    fn economy_state(&self) -> VarDictionary {
        let summary = self.city_summary();
        let population = summary
            .get("population")
            .map_or(0, |value| value.to::<i64>()) as u64;
        let employed = summary.get("employed").map_or(0, |value| value.to::<i64>()) as u64;
        let buildings = summary
            .get("completed_buildings")
            .map_or(0, |value| value.to::<i64>()) as u64;
        let guard = self
            .economy
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut result = VarDictionary::new();
        result.set("ok", true);
        match guard.as_ref() {
            Some(economy) => {
                result.set("open", true);
                result.set("seed", economy.seed as i64);
                result.set("balance", economy.balance);
                result.set("tax_per_resident_per_day", economy.tax_per_resident_per_day);
                result.set(
                    "business_tax_per_job_per_day",
                    economy.business_tax_per_job_per_day,
                );
                result.set(
                    "upkeep_per_building_per_day",
                    economy.upkeep_per_building_per_day,
                );
                result.set("construction_spent", economy.construction_spent);
                result.set("days_elapsed", economy.days_elapsed);
                result.set(
                    "daily_net",
                    economy.daily_net(population, employed, buildings),
                );
                result.set("bankrupt", economy.bankrupt);
                result.set(
                    "checksum",
                    &GString::from(&format!("{:016x}", economy.checksum())),
                );
            }
            None => {
                result.set("open", false);
                result.set("seed", 0);
                result.set("balance", 0.0);
                result.set("bankrupt", false);
                result.set("checksum", &GString::from("0000000000000000"));
            }
        }
        result
    }

    /// Advance taxes, wages, and upkeep by `seconds`.
    #[func]
    fn economy_tick(&self, seed: i64, seconds: f64) -> VarDictionary {
        let Ok(seed) = u64::try_from(seed) else {
            return placement_simple_error("invalid_seed", "world seed must be non-negative");
        };
        if !seconds.is_finite() || !(0.0..=core_city::MAX_ECONOMY_TICK_SECONDS).contains(&seconds) {
            return placement_simple_error("invalid_delta", "economy delta is outside 0..=86400");
        }
        let summary = self.city_summary();
        let population = summary
            .get("population")
            .map_or(0, |value| value.to::<i64>()) as u64;
        let employed = summary.get("employed").map_or(0, |value| value.to::<i64>()) as u64;
        let buildings = summary
            .get("completed_buildings")
            .map_or(0, |value| value.to::<i64>()) as u64;
        let mut guard = self
            .economy
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if guard.as_ref().is_some_and(|economy| economy.seed != seed) {
            return placement_simple_error(
                "seed_mismatch",
                "the treasury belongs to a different seed",
            );
        }
        let economy = guard.get_or_insert_with(|| Economy::new(seed));
        let tick = economy.tick(seconds, population, employed, buildings);
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("income", tick.income);
        result.set("expenses", tick.expenses);
        result.set("balance", tick.balance);
        result.set("days_elapsed", tick.days_elapsed);
        result.set("bankrupt", tick.bankrupt);
        result.set(
            "checksum",
            &GString::from(&format!("{:016x}", economy.checksum())),
        );
        result
    }

    /// Set the resident and business tax rates.
    #[func]
    fn economy_set_taxes(&self, seed: i64, resident: f64, business: f64) -> VarDictionary {
        let Ok(seed) = u64::try_from(seed) else {
            return placement_simple_error("invalid_seed", "world seed must be non-negative");
        };
        {
            let world = self.world.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(store) = world.as_ref() {
                if store.seed != seed {
                    return placement_simple_error(
                        "seed_mismatch",
                        "the placement store belongs to a different seed",
                    );
                }
            }
        }
        let mut guard = self
            .economy
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if guard.as_ref().is_some_and(|economy| economy.seed != seed) {
            return placement_simple_error(
                "seed_mismatch",
                "the treasury belongs to a different seed",
            );
        }
        {
            let economy = guard.get_or_insert_with(|| Economy::new(seed));
            economy.set_taxes(resident, business);
        }
        drop(guard);
        self.economy_state()
    }

    /// Stable treasury checksum.
    #[func]
    fn economy_checksum(&self) -> GString {
        let guard = self
            .economy
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let checksum = guard.as_ref().map_or(0, Economy::checksum);
        GString::from(&format!("{checksum:016x}"))
    }

    /// Clear the treasury and keep no open economy.
    #[func]
    fn economy_clear(&self) -> bool {
        *self
            .economy
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        true
    }

    /// Designate a placement as an industry site.
    ///
    /// Returns `{ ok, placement_id, kind }`; the placement must exist and have
    /// at least one job slot.
    #[func]
    fn industry_add_site(&self, seed: i64, placement_id: i64, kind: GString) -> VarDictionary {
        let Ok(seed) = u64::try_from(seed) else {
            return placement_simple_error("invalid_seed", "world seed must be non-negative");
        };
        let Ok(placement_id) = u64::try_from(placement_id) else {
            return placement_simple_error(
                "invalid_placement",
                "placement id must be non-negative",
            );
        };
        let Some(kind) = IndustryKind::parse(&kind.to_string()) else {
            return placement_simple_error(
                "invalid_industry",
                "industry kind must be lumberjack, farm, or hunt",
            );
        };
        {
            let world = self.world.lock().unwrap_or_else(|error| error.into_inner());
            let Some(store) = world.as_ref() else {
                return placement_simple_error("no_world", "no world placement store is open");
            };
            if store.seed != seed {
                return placement_simple_error(
                    "seed_mismatch",
                    "the placement store belongs to a different seed",
                );
            }
            let Some(placement) = store.placements.iter().find(|item| item.id == placement_id)
            else {
                return placement_simple_error("unknown_placement", "no placement has that id");
            };
            if placement.job_slots == 0 {
                return placement_simple_error(
                    "no_jobs",
                    "an industry site needs at least one job slot",
                );
            }
        }
        let mut guard = self
            .industry
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if guard.as_ref().is_some_and(|industry| industry.seed != seed) {
            return placement_simple_error(
                "seed_mismatch",
                "the industry store belongs to a different seed",
            );
        }
        let industry = guard.get_or_insert_with(|| IndustryStore::new(seed));
        if let Err(error) = industry.add_site(placement_id, kind) {
            return placement_simple_error("industry_failed", error.to_string());
        }
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("placement_id", placement_id as i64);
        result.set("kind", &GString::from(kind.as_str()));
        result
    }

    /// Remove one industry site.
    #[func]
    fn industry_remove_site(&self, placement_id: i64) -> bool {
        let Ok(placement_id) = u64::try_from(placement_id) else {
            return false;
        };
        self.industry
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_mut()
            .is_some_and(|industry| industry.remove_site(placement_id))
    }

    /// Return industry sites, staffing, stockpile, and production.
    #[func]
    fn industry_state(&self) -> VarDictionary {
        let (industry_seed, site_ids): (Option<u64>, BTreeSet<u64>) = {
            let industry = self
                .industry
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            industry.as_ref().map_or_else(
                || (None, BTreeSet::new()),
                |industry| {
                    (
                        Some(industry.seed),
                        industry.sites.keys().copied().collect(),
                    )
                },
            )
        };
        let centers: BTreeMap<u64, [f32; 3]> = {
            let world = self.world.lock().unwrap_or_else(|error| error.into_inner());
            world.as_ref().map_or_else(BTreeMap::new, |store| {
                store
                    .placements
                    .iter()
                    .filter(|placement| site_ids.contains(&placement.id))
                    .map(|placement| (placement.id, placement.center))
                    .collect()
            })
        };
        let staffing: BTreeMap<u64, u32> = {
            let population = self
                .population
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            population
                .as_ref()
                .filter(|runtime| Some(runtime.seed) == industry_seed)
                .map_or_else(BTreeMap::new, |runtime| runtime.staffing.clone())
        };
        let guard = self
            .industry
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut result = VarDictionary::new();
        result.set("ok", true);
        match guard.as_ref() {
            Some(industry) => {
                result.set("open", true);
                result.set("seed", industry.seed as i64);
                result.set("site_count", industry.site_count() as i64);
                result.set("wood", industry.stockpile.wood);
                result.set("food", industry.stockpile.food);
                result.set("hides", industry.stockpile.hides);
                result.set("produced_wood", industry.produced_total.wood);
                result.set("produced_food", industry.produced_total.food);
                result.set("produced_hides", industry.produced_total.hides);
                result.set("food_supply", f64::from(industry.food_supply));
                let mut sites: Array<VarDictionary> = Array::new();
                for (&id, site) in &industry.sites {
                    let mut item = VarDictionary::new();
                    item.set("placement_id", id as i64);
                    item.set("kind", &GString::from(site.kind.as_str()));
                    item.set("produced", site.produced);
                    item.set(
                        "workers",
                        i64::from(staffing.get(&id).copied().unwrap_or(0)),
                    );
                    let center = centers.get(&id).copied().unwrap_or([0.0, 40.0, 0.0]);
                    item.set("center_x", f64::from(center[0]));
                    item.set("center_y", f64::from(center[1]));
                    item.set("center_z", f64::from(center[2]));
                    sites.push(&item);
                }
                result.set("sites", &sites.to_variant());
                result.set(
                    "checksum",
                    &GString::from(&format!("{:016x}", industry.checksum())),
                );
            }
            None => {
                result.set("open", false);
                result.set("seed", 0);
                result.set("site_count", 0);
                result.set("wood", 0.0);
                result.set("food", 0.0);
                result.set("hides", 0.0);
                result.set("produced_wood", 0.0);
                result.set("produced_food", 0.0);
                result.set("produced_hides", 0.0);
                result.set("food_supply", 0.0);
                result.set("sites", &Array::<VarDictionary>::new().to_variant());
                result.set("checksum", &GString::from("0000000000000000"));
            }
        }
        result
    }

    /// Advance industry production and food consumption by `seconds`.
    #[func]
    fn industry_tick(&self, seed: i64, seconds: f64) -> VarDictionary {
        let Ok(seed) = u64::try_from(seed) else {
            return placement_simple_error("invalid_seed", "world seed must be non-negative");
        };
        if !seconds.is_finite()
            || !(0.0..=core_city::MAX_POPULATION_TICK_SECONDS).contains(&seconds)
        {
            return placement_simple_error("invalid_delta", "industry delta is outside 0..=3600");
        }
        {
            let world = self.world.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(store) = world.as_ref() {
                if store.seed != seed {
                    return placement_simple_error(
                        "seed_mismatch",
                        "the placement store belongs to a different seed",
                    );
                }
            }
        }
        let (staffing, population) = {
            let runtime = self
                .population
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            runtime
                .as_ref()
                .filter(|runtime| runtime.seed == seed)
                .map_or_else(
                    || (BTreeMap::new(), 0),
                    |runtime| (runtime.staffing.clone(), runtime.population()),
                )
        };
        let mut guard = self
            .industry
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(industry) = guard.as_mut() else {
            let mut result = VarDictionary::new();
            result.set("ok", true);
            result.set("open", false);
            result.set("wood", 0.0);
            result.set("food", 0.0);
            result.set("hides", 0.0);
            result.set("food_consumed", 0.0);
            result.set("food_supply", 0.0);
            result.set("workers", 0);
            result.set("checksum", &GString::from("0000000000000000"));
            return result;
        };
        if industry.seed != seed {
            return placement_simple_error(
                "seed_mismatch",
                "the industry store belongs to a different seed",
            );
        }
        let tick = industry.tick(seconds, &staffing, population);
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("open", true);
        result.set("wood", tick.wood);
        result.set("food", tick.food);
        result.set("hides", tick.hides);
        result.set("food_consumed", tick.food_consumed);
        result.set("food_supply", f64::from(tick.food_supply));
        result.set("workers", tick.workers as i64);
        result.set("stock_wood", tick.stockpile.wood);
        result.set("stock_food", tick.stockpile.food);
        result.set("stock_hides", tick.stockpile.hides);
        result.set(
            "checksum",
            &GString::from(&format!("{:016x}", industry.checksum())),
        );
        result
    }

    /// Stable industry checksum.
    #[func]
    fn industry_checksum(&self) -> GString {
        let guard = self
            .industry
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let checksum = guard.as_ref().map_or(0, IndustryStore::checksum);
        GString::from(&format!("{checksum:016x}"))
    }

    /// Close the industry store.
    #[func]
    fn industry_clear(&self) -> bool {
        *self
            .industry
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        true
    }

    /// Set the shared crew pool. Values above 64 are rejected.
    #[func]
    fn construction_set_crew(&self, crew_capacity: i64) -> bool {
        let Ok(crew_capacity) = u16::try_from(crew_capacity) else {
            return false;
        };
        if crew_capacity > 64 {
            return false;
        }
        self.construction
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .set_crew_capacity(crew_capacity);
        true
    }

    /// Add material litres to the shared stockpile.
    #[func]
    fn construction_add_materials(&self, litres: i64) -> bool {
        let Ok(litres) = u64::try_from(litres) else {
            return false;
        };
        self.construction
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .deliver_materials(litres);
        true
    }

    /// Stable construction checksum.
    #[func]
    fn construction_checksum(&self) -> GString {
        let store = self
            .construction
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        GString::from(&format!("{:016x}", store.checksum()))
    }

    /// Remove every construction site. Sites are also cleared with placements.
    #[func]
    fn construction_clear(&self) -> bool {
        self.construction
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        true
    }

    /// Write the placement store as JSON.
    #[func]
    fn world_save(&self, path: GString) -> VarDictionary {
        let path = match world_save_path_from_arg(&path.to_string()) {
            Ok(path) => path,
            Err(error) => return placement_simple_error("invalid_save_path", error),
        };
        let (store, construction, roads, lots, settlements, mut population, economy, industry) = {
            let world = self.world.lock().unwrap_or_else(|error| error.into_inner());
            let construction = self
                .construction
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let roads = self.roads.lock().unwrap_or_else(|error| error.into_inner());
            let lots = self.lots.lock().unwrap_or_else(|error| error.into_inner());
            let settlements = self
                .settlements
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let population = self
                .population
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let economy = self
                .economy
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let industry = self
                .industry
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            (
                world.as_ref().cloned(),
                construction.clone(),
                roads.as_ref().cloned(),
                lots.as_ref().cloned(),
                settlements.as_ref().cloned(),
                population.as_ref().cloned(),
                economy.as_ref().cloned(),
                industry.as_ref().cloned(),
            )
        };
        let Some(store) = store else {
            return placement_simple_error("no_world", "no world placement store is open");
        };
        if let Some(runtime) = population.as_mut() {
            runtime.sync(&completed_slots(&store, &construction));
            sanitize_staffing(runtime, &store, &construction);
            if let Err(error) = runtime.validate() {
                return placement_simple_error("invalid_population", error.to_string());
            }
        }
        let json = match store.to_json_pretty() {
            Ok(json) => json,
            Err(error) => return placement_error_result(&error),
        };
        let (deltas_bytes, deltas_checksum, deltas_count) = {
            let deltas = self
                .terrain_deltas
                .read()
                .unwrap_or_else(|error| error.into_inner());
            (deltas.encode(), deltas.checksum(), deltas.edit_count())
        };
        if deltas_count > 0 {
            let deltas_seed = *self
                .terrain_seed
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if deltas_seed != Some(store.seed) {
                return placement_simple_error(
                    "seed_mismatch",
                    "terrain deltas belong to a different seed",
                );
            }
        }
        let json = if deltas_count > 0 {
            match attach_terrain_deltas(&json, &deltas_bytes, deltas_checksum) {
                Ok(json) => json,
                Err(error) => return placement_simple_error("save_failed", error),
            }
        } else {
            json
        };
        let json = match attach_construction(&json, &construction) {
            Ok(json) => json,
            Err(error) => return placement_simple_error("save_failed", error),
        };
        let json = match population.as_ref() {
            Some(population) => {
                if population.seed != store.seed {
                    return placement_simple_error(
                        "seed_mismatch",
                        "households belong to a different seed",
                    );
                }
                match attach_population(&json, population) {
                    Ok(json) => json,
                    Err(error) => return placement_simple_error("save_failed", error),
                }
            }
            None => json,
        };
        let json = match roads.as_ref() {
            Some(roads) => {
                if roads.seed != store.seed {
                    return placement_simple_error(
                        "seed_mismatch",
                        "roads belong to a different seed",
                    );
                }
                match attach_roads(&json, roads) {
                    Ok(json) => json,
                    Err(error) => return placement_simple_error("save_failed", error),
                }
            }
            None => json,
        };
        let json = match lots.as_ref() {
            Some(lots) => {
                if lots.seed != store.seed {
                    return placement_simple_error(
                        "seed_mismatch",
                        "lots belong to a different seed",
                    );
                }
                match attach_lots(&json, lots) {
                    Ok(json) => json,
                    Err(error) => return placement_simple_error("save_failed", error),
                }
            }
            None => json,
        };
        let json = match settlements.as_ref() {
            Some(settlements) => {
                if settlements.seed != store.seed {
                    return placement_simple_error(
                        "seed_mismatch",
                        "settlements belong to a different seed",
                    );
                }
                match attach_settlements(&json, settlements) {
                    Ok(json) => json,
                    Err(error) => return placement_simple_error("save_failed", error),
                }
            }
            None => json,
        };
        let json = match economy.as_ref() {
            Some(economy) => {
                if economy.seed != store.seed {
                    return placement_simple_error(
                        "seed_mismatch",
                        "the treasury belongs to a different seed",
                    );
                }
                match attach_economy(&json, economy) {
                    Ok(json) => json,
                    Err(error) => return placement_simple_error("save_failed", error),
                }
            }
            None => json,
        };
        let json = match industry.as_ref() {
            Some(industry) => {
                if industry.seed != store.seed {
                    return placement_simple_error(
                        "seed_mismatch",
                        "the industry store belongs to a different seed",
                    );
                }
                match attach_industry(&json, industry) {
                    Ok(json) => json,
                    Err(error) => return placement_simple_error("save_failed", error),
                }
            }
            None => json,
        };
        if json.len() > MAX_WORLD_SAVE_BYTES as usize {
            return placement_simple_error(
                "save_too_large",
                format!(
                    "world save is {} bytes; the limit is {MAX_WORLD_SAVE_BYTES}",
                    json.len()
                ),
            );
        }
        if let Err(error) = write_text_atomic(&path, &json) {
            return placement_simple_error("save_failed", error.to_string());
        }
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("path", &GString::from(path.to_string_lossy().as_ref()));
        result.set("seed", store.seed as i64);
        result.set("count", store.placements.len() as i64);
        result.set("terrain_edits", deltas_count as i64);
        result.set("construction_sites", construction.site_count() as i64);
        result.set(
            "roads",
            roads.as_ref().map_or(0, |roads| roads.roads.len()) as i64,
        );
        result.set(
            "lots",
            lots.as_ref().map_or(0, |lots| lots.lots.len()) as i64,
        );
        result.set(
            "settlements",
            settlements.as_ref().map_or(0, |store| store.count()) as i64,
        );
        result.set(
            "checksum",
            &GString::from(&format!("{:016x}", store.checksum())),
        );
        result.set(
            "construction_checksum",
            &GString::from(&format!("{:016x}", construction.checksum())),
        );
        result.set(
            "population",
            population.as_ref().map_or(0, CityRuntime::population) as i64,
        );
        result.set(
            "population_checksum",
            &GString::from(&format!(
                "{:016x}",
                population.as_ref().map_or(0, CityRuntime::checksum)
            )),
        );
        result.set(
            "road_checksum",
            &GString::from(&format!(
                "{:016x}",
                roads.as_ref().map_or(0, RoadStore::checksum)
            )),
        );
        result.set(
            "lot_checksum",
            &GString::from(&format!(
                "{:016x}",
                lots.as_ref().map_or(0, LotStore::checksum)
            )),
        );
        result.set(
            "settlement_checksum",
            &GString::from(&format!(
                "{:016x}",
                settlements.as_ref().map_or(0, SettlementStore::checksum)
            )),
        );
        result.set(
            "industry_sites",
            industry.as_ref().map_or(0, IndustryStore::site_count) as i64,
        );
        result.set(
            "industry_checksum",
            &GString::from(&format!(
                "{:016x}",
                industry.as_ref().map_or(0, IndustryStore::checksum)
            )),
        );
        result.set(
            "terrain_checksum",
            &GString::from(&format!("{deltas_checksum:016x}")),
        );
        result
    }

    /// Read a world placement save for `expected_seed` and verify its packages.
    #[func]
    fn world_load(&self, path: GString, expected_seed: i64) -> VarDictionary {
        let expected_seed = match u64::try_from(expected_seed) {
            Ok(seed) => seed,
            Err(_) => {
                return placement_simple_error("invalid_seed", "world seed must be non-negative")
            }
        };
        let path = match world_save_path_from_arg(&path.to_string()) {
            Ok(path) => path,
            Err(error) => return placement_simple_error("invalid_save_path", error),
        };
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => return placement_simple_error("load_failed", error.to_string()),
        };
        if metadata.len() > MAX_WORLD_SAVE_BYTES {
            return placement_simple_error(
                "save_too_large",
                format!(
                    "world save is {} bytes; the limit is {MAX_WORLD_SAVE_BYTES}",
                    metadata.len()
                ),
            );
        }
        let json = match std::fs::read_to_string(&path) {
            Ok(json) => json,
            Err(error) => return placement_simple_error("load_failed", error.to_string()),
        };
        let deltas = match terrain_deltas_from_world_json(&json) {
            Ok(deltas) => deltas,
            Err((code, error)) => return placement_simple_error(code, error),
        };
        let mut construction = match construction_from_world_json(&json) {
            Ok(Some(store)) => store,
            Ok(None) => ConstructionStore::default(),
            Err((code, error)) => return placement_simple_error(code, error),
        };
        let mut population = match population_from_world_json(&json) {
            Ok(Some(runtime)) => runtime,
            Ok(None) => CityRuntime::new(expected_seed),
            Err((code, error)) => return placement_simple_error(code, error),
        };
        if population.seed != expected_seed {
            return placement_simple_error(
                "seed_mismatch",
                format!(
                    "households use seed {} but this world is {expected_seed}",
                    population.seed
                ),
            );
        }
        let roads = match roads_from_world_json(&json) {
            Ok(Some(store)) => Some(store),
            Ok(None) => None,
            Err((code, error)) => return placement_simple_error(code, error),
        };
        if let Some(roads) = roads.as_ref() {
            if roads.seed != expected_seed {
                return placement_simple_error(
                    "seed_mismatch",
                    format!(
                        "roads use seed {} but this world is {expected_seed}",
                        roads.seed
                    ),
                );
            }
        }
        let lots = match lots_from_world_json(&json) {
            Ok(Some(store)) => Some(store),
            Ok(None) => None,
            Err((code, error)) => return placement_simple_error(code, error),
        };
        if let Some(lots) = lots.as_ref() {
            if lots.seed != expected_seed {
                return placement_simple_error(
                    "seed_mismatch",
                    format!(
                        "lots use seed {} but this world is {expected_seed}",
                        lots.seed
                    ),
                );
            }
        }
        let settlements = match settlements_from_world_json(&json) {
            Ok(Some(store)) => Some(store),
            Ok(None) => None,
            Err((code, error)) => return placement_simple_error(code, error),
        };
        if let Some(settlements) = settlements.as_ref() {
            if settlements.seed != expected_seed {
                return placement_simple_error(
                    "seed_mismatch",
                    format!(
                        "settlements use seed {} but this world is {expected_seed}",
                        settlements.seed
                    ),
                );
            }
        }
        let economy = match economy_from_world_json(&json) {
            Ok(economy) => economy,
            Err((code, error)) => return placement_simple_error(code, error),
        };
        if let Some(economy) = economy.as_ref() {
            if economy.seed != expected_seed {
                return placement_simple_error(
                    "seed_mismatch",
                    format!(
                        "the treasury uses seed {} but this world is {expected_seed}",
                        economy.seed
                    ),
                );
            }
        }
        let industry = match industry_from_world_json(&json) {
            Ok(industry) => industry,
            Err((code, error)) => return placement_simple_error(code, error),
        };
        if let Some(industry) = industry.as_ref() {
            if industry.seed != expected_seed {
                return placement_simple_error(
                    "seed_mismatch",
                    format!(
                        "the industry store uses seed {} but this world is {expected_seed}",
                        industry.seed
                    ),
                );
            }
        }
        let store = match PlacementStore::from_json(&json) {
            Ok(store) => store,
            Err(error) => return placement_error_result(&error),
        };
        if store.seed != expected_seed {
            return placement_simple_error(
                "seed_mismatch",
                format!(
                    "save uses seed {} but this world is {expected_seed}",
                    store.seed
                ),
            );
        }
        let mut solid_by_id: BTreeMap<u64, u64> = BTreeMap::new();
        for placement in &store.placements {
            let package_path = match blueprint_path_from_arg(&placement.blueprint_path) {
                Ok(path) => path,
                Err(error) => {
                    return placement_simple_error(
                        "invalid_blueprint_path",
                        format!("{}: {error}", placement.blueprint_path),
                    )
                }
            };
            let package = match load_tsbp(&package_path) {
                Ok(package) => package,
                Err(error) => {
                    return placement_simple_error(
                        blueprint_error_code(&error),
                        format!("{}: {error}", placement.blueprint_path),
                    )
                }
            };
            if package.metadata.content_hash != placement.blueprint_hash {
                return placement_simple_error(
                    "blueprint_hash_mismatch",
                    format!(
                        "{} expected {} but the package is {}",
                        placement.blueprint_path,
                        placement.blueprint_hash,
                        package.metadata.content_hash
                    ),
                );
            }
            solid_by_id.insert(placement.id, package.metadata.solid_voxels);
            if package.metadata.id != placement.blueprint_id
                || package.metadata.name != placement.name
                || package.metadata.bounds_m != placement.bounds_m
                || package.metadata.stats.resident_capacity != placement.resident_capacity
                || package.metadata.stats.job_slots != placement.job_slots
            {
                return placement_simple_error(
                    "blueprint_metadata_mismatch",
                    format!(
                        "{} metadata does not match the placement record",
                        placement.blueprint_path
                    ),
                );
            }
        }
        let placement_ids: BTreeSet<u64> = store
            .placements
            .iter()
            .map(|placement| placement.id)
            .collect();
        for placement in &store.placements {
            if construction.site(placement.id).is_none() {
                let solid_voxels = if placement.solid_voxels > 0 {
                    placement.solid_voxels
                } else {
                    *solid_by_id.get(&placement.id).unwrap_or(&1)
                };
                if let Err(error) = construction.add_site(placement.id, solid_voxels) {
                    return placement_simple_error("construction_failed", error.to_string());
                }
            }
        }
        for id in construction.sites.keys() {
            if !placement_ids.contains(id) {
                return placement_simple_error(
                    "construction_orphan",
                    format!("construction site {id} has no placement"),
                );
            }
        }
        if let Err(error) = construction.validate() {
            return placement_simple_error("invalid_construction", error.to_string());
        }
        if let Some(industry) = industry.as_ref() {
            for &id in industry.sites.keys() {
                let Some(placement) = store.placements.iter().find(|item| item.id == id) else {
                    return placement_simple_error(
                        "industry_orphan",
                        format!("industry site {id} has no placement"),
                    );
                };
                if placement.job_slots == 0 {
                    return placement_simple_error(
                        "industry_no_jobs",
                        format!("industry site {id} has no job slots"),
                    );
                }
            }
        }
        population.sync(&completed_slots(&store, &construction));
        sanitize_staffing(&mut population, &store, &construction);
        if let Err(error) = population.validate() {
            return placement_simple_error("invalid_population", error.to_string());
        }
        let population_count = population.population();
        let population_checksum = population.checksum();
        let seed = store.seed;
        let count = store.placements.len() as i64;
        let checksum = store.checksum();
        let construction_count = construction.site_count();
        let construction_checksum = construction.checksum();
        let road_count = roads.as_ref().map_or(0, |roads| roads.roads.len());
        let road_checksum = roads.as_ref().map_or(0, RoadStore::checksum);
        let lot_count = lots.as_ref().map_or(0, |lots| lots.lots.len());
        let lot_checksum = lots.as_ref().map_or(0, LotStore::checksum);
        let settlement_count = settlements.as_ref().map_or(0, |store| store.count());
        let settlement_checksum = settlements.as_ref().map_or(0, SettlementStore::checksum);
        let industry_count = industry.as_ref().map_or(0, IndustryStore::site_count);
        let industry_checksum = industry.as_ref().map_or(0, IndustryStore::checksum);
        let deltas_count = deltas.edit_count();
        let deltas_checksum = deltas.checksum();
        let mut guard = self.world.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(current) = guard.as_ref() {
            if current.seed != seed {
                return placement_simple_error(
                    "seed_mismatch",
                    "the loaded save belongs to a different seed",
                );
            }
        }
        let mut construction_guard = self
            .construction
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut roads_guard = self.roads.lock().unwrap_or_else(|error| error.into_inner());
        let mut lots_guard = self.lots.lock().unwrap_or_else(|error| error.into_inner());
        let mut settlements_guard = self
            .settlements
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut population_guard = self
            .population
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut economy_guard = self
            .economy
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut industry_guard = self
            .industry
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut deltas_guard = self
            .terrain_deltas
            .write()
            .unwrap_or_else(|error| error.into_inner());
        *construction_guard = construction;
        *roads_guard = roads;
        *lots_guard = lots;
        *settlements_guard = settlements;
        *guard = Some(store);
        *population_guard = Some(population);
        *economy_guard = economy;
        *industry_guard = industry;
        *deltas_guard = deltas;
        *self
            .terrain_seed
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            if deltas_count > 0 { Some(seed) } else { None };
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("path", &GString::from(path.to_string_lossy().as_ref()));
        result.set("seed", seed as i64);
        result.set("count", count);
        result.set("terrain_edits", deltas_count as i64);
        result.set("construction_sites", construction_count as i64);
        result.set("roads", road_count as i64);
        result.set("lots", lot_count as i64);
        result.set("settlements", settlement_count as i64);
        result.set("checksum", &GString::from(&format!("{checksum:016x}")));
        result.set(
            "terrain_checksum",
            &GString::from(&format!("{deltas_checksum:016x}")),
        );
        result.set(
            "construction_checksum",
            &GString::from(&format!("{construction_checksum:016x}")),
        );
        result.set("population", population_count as i64);
        result.set(
            "population_checksum",
            &GString::from(&format!("{population_checksum:016x}")),
        );
        result.set(
            "road_checksum",
            &GString::from(&format!("{road_checksum:016x}")),
        );
        result.set(
            "lot_checksum",
            &GString::from(&format!("{lot_checksum:016x}")),
        );
        result.set(
            "settlement_checksum",
            &GString::from(&format!("{settlement_checksum:016x}")),
        );
        result.set("industry_sites", industry_count as i64);
        result.set(
            "industry_checksum",
            &GString::from(&format!("{industry_checksum:016x}")),
        );
        result
    }

    /// Set one terrain cell to a new height and record the delta.
    ///
    /// Returns `{ ok, chunk_x, chunk_z, height, edits }`. The shell remeshes
    /// the returned chunk. Heights must be 0..=255 m.
    #[func]
    fn terrain_edit_height(&self, seed: i64, x: i64, z: i64, height: i64) -> VarDictionary {
        let Ok(seed) = u64::try_from(seed) else {
            return placement_simple_error("invalid_seed", "world seed must be non-negative");
        };
        if x.unsigned_abs() > 1_000_000 || z.unsigned_abs() > 1_000_000 {
            return placement_simple_error(
                "coordinate_out_of_range",
                "terrain edit coordinates must be within ±1,000,000 m",
            );
        }
        let Ok(height) = u16::try_from(height) else {
            return placement_simple_error("invalid_height", "terrain height must be 0..=255 m");
        };
        if height > MAX_HEIGHT_M {
            return placement_simple_error("invalid_height", "terrain height must be 0..=255 m");
        }
        let pos = TerrainPos::new(x, z);
        let coord = WorldChunkCoord::from_terrain_pos(pos);
        let Some((local_x, local_z)) = coord.local_from_world(pos) else {
            return placement_simple_error(
                "invalid_coordinate",
                "terrain position is out of range",
            );
        };
        let mut world = self.world.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(store) = world.as_ref() {
            if store.seed != seed {
                return placement_simple_error(
                    "seed_mismatch",
                    "the world belongs to a different seed",
                );
            }
        }
        let mut deltas = self
            .terrain_deltas
            .write()
            .unwrap_or_else(|error| error.into_inner());
        if let Err(error) = deltas.set_height(coord, local_x as u8, local_z as u8, height) {
            return placement_simple_error("edit_failed", error.to_string());
        }
        if world.is_none() {
            *world = Some(PlacementStore::new(seed));
        }
        *self
            .terrain_seed
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(seed);
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("chunk_x", coord.x);
        result.set("chunk_z", coord.z);
        result.set("local_x", local_x as i64);
        result.set("local_z", local_z as i64);
        result.set("height", i64::from(height));
        result.set("edits", deltas.edit_count() as i64);
        result
    }

    /// Number of terrain edits.
    #[func]
    fn terrain_delta_count(&self) -> i64 {
        self.terrain_deltas
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .edit_count() as i64
    }

    /// Stable checksum of the encoded terrain deltas.
    #[func]
    fn terrain_delta_checksum(&self) -> GString {
        let checksum = self
            .terrain_deltas
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .checksum();
        GString::from(&format!("{checksum:016x}"))
    }

    /// Seed that owns the terrain edits, or `-1` when there are none.
    #[func]
    fn terrain_deltas_seed(&self) -> i64 {
        self.terrain_seed
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .map_or(-1, |seed| seed as i64)
    }

    /// Remove every terrain edit.
    #[func]
    fn terrain_clear_deltas(&self) -> bool {
        *self
            .terrain_deltas
            .write()
            .unwrap_or_else(|error| error.into_inner()) = DeltaStore::new();
        *self
            .terrain_seed
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        true
    }

    /// Sample one terrain height for the placement preview.
    ///
    /// Uses recorded edits when they belong to `seed`. Returns `-1.0` for
    /// invalid coordinates.
    #[func]
    fn world_height_at(&self, seed: i64, x: f64, z: f64) -> f64 {
        let Ok(seed) = u64::try_from(seed) else {
            return -1.0;
        };
        if !x.is_finite() || !z.is_finite() {
            return -1.0;
        }
        if x.abs() > f64::from(MAX_WORLD_COORD_M) || z.abs() > f64::from(MAX_WORLD_COORD_M) {
            return -1.0;
        }
        f64::from(
            self.terrain_height_for_seed(seed, TerrainPos::new(x.floor() as i64, z.floor() as i64)),
        )
    }

    /// Create or replace the editor [`VoxelVolume`].
    ///
    /// `chunks_x`, `chunks_y`, and `chunks_z` count 32³ chunks. Pass
    /// `0, 0, 0` to select the default 5 × 3 × 5 volume
    /// (16 m × 9.6 m × 16 m). Mixed zero axes, negative values, an axis above
    /// [`MAX_CHUNKS_PER_AXIS`], or more than [`MAX_CHUNKS`] total chunks are
    /// rejected. A rejected call returns `false` and leaves an existing volume
    /// untouched.
    ///
    /// **Main thread only.** This method creates no Godot resources, but it
    /// mutates the bridge state and is part of the single-threaded editor API.
    #[func]
    fn create_editor_volume(&self, chunks_x: i64, chunks_y: i64, chunks_z: i64) -> bool {
        let config = match editor_config_from_args(chunks_x, chunks_y, chunks_z) {
            Ok(config) => config,
            Err(message) => {
                eprintln!("Townsmith: {message}");
                return false;
            }
        };
        let volume = match VoxelVolume::new(config) {
            Ok(volume) => volume,
            Err(error) => {
                eprintln!("Townsmith: invalid editor volume: {error}");
                return false;
            }
        };

        let mut guard = self
            .editor
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *guard = Some(EditorState::new(volume));
        true
    }

    /// Voxel configuration of the current editor volume.
    ///
    /// Returns a dictionary with `created` (`bool`), `chunks` and `voxels`
    /// (`PackedInt32Array`, `[x, y, z]`), `bounds_m` (`PackedFloat32Array`),
    /// `chunk_count` (`int`), `chunk_size` (`int`), and `voxel_size_m`
    /// (`float`). Without a volume it returns
    /// `{ "created": false, "error": "no_editor_volume" }`.
    ///
    /// **Main thread only.**
    #[func]
    fn editor_volume_size_voxels(&self) -> VarDictionary {
        let mut result = VarDictionary::new();
        let guard = self
            .editor
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(state) = guard.as_ref() else {
            result.set("created", false);
            result.set("error", "no editor volume is open");
            return result;
        };

        result.set("created", true);
        volume_config_to_dictionary(state.volume.config(), &mut result);
        result
    }

    /// Volume extents in meters as `[x, y, z]`.
    ///
    /// Returns an empty `PackedFloat32Array` when no editor volume exists.
    ///
    /// **Main thread only.**
    #[func]
    fn editor_volume_bounds(&self) -> PackedFloat32Array {
        let guard = self
            .editor
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        guard
            .as_ref()
            .map_or_else(PackedFloat32Array::new, |state| {
                PackedFloat32Array::from(state.volume.config().bounds_m().to_vec())
            })
    }

    /// Write one 10 cm voxel in the editor volume.
    ///
    /// `material` is `0..=65535`; `0` clears the voxel. Negative or
    /// out-of-volume coordinates, a missing volume, and out-of-range materials
    /// return `false` without changing anything. A successful write marks the
    /// current mesh stats stale until [`Self::build_editor_volume_mesh`] runs.
    ///
    /// **Main thread only.**
    #[func]
    fn editor_set_voxel(&self, x: i64, y: i64, z: i64, material: i64) -> bool {
        let Some(pos) = volume_pos_from_args(x, y, z) else {
            return false;
        };
        let Some(material) = material_from_args(material) else {
            return false;
        };

        let mut guard = self
            .editor
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(state) = guard.as_mut() else {
            return false;
        };
        if state.volume.set_voxel(pos, material).is_err() {
            return false;
        }
        state.mark_dirty();
        true
    }

    /// Fill a half-open box `[min, max)` in the editor volume.
    ///
    /// The box is validated before anything is written, so a rejected call
    /// leaves the volume unchanged. `max` is exclusive, matching
    /// [`VoxelVolume::fill_box`]. Material and error behavior matches
    /// [`Self::editor_set_voxel`].
    ///
    /// **Main thread only.**
    // GDScript call sites pass six coordinates plus the material; a struct
    // argument would make the Godot API awkward and slower to call.
    #[allow(clippy::too_many_arguments)]
    #[func]
    fn editor_fill_box(
        &self,
        min_x: i64,
        min_y: i64,
        min_z: i64,
        max_x: i64,
        max_y: i64,
        max_z: i64,
        material: i64,
    ) -> bool {
        let Some(min) = volume_pos_from_args(min_x, min_y, min_z) else {
            return false;
        };
        let Some(max) = volume_pos_from_args(max_x, max_y, max_z) else {
            return false;
        };
        let Some(material) = material_from_args(material) else {
            return false;
        };

        let mut guard = self
            .editor
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(state) = guard.as_mut() else {
            return false;
        };
        if state.volume.fill_box(min, max, material).is_err() {
            return false;
        }
        state.mark_dirty();
        true
    }

    /// Replace the editor volume with the deterministic acceptance house.
    ///
    /// The fixture lives in `core_rooms::demo`: 8 m × 6 m × 6 m, two floors,
    /// kitchen, bathroom, two bedrooms, doors, windows, and eight props. It is
    /// shared by the room benchmark and the end-to-end slice smoke so the
    /// acceptance numbers cannot drift between them. This always installs the
    /// default 5 × 3 × 5 chunk volume (16 m × 9.6 m × 16 m).
    ///
    /// Returns `false` when no editor volume exists.
    ///
    /// **Main thread only.**
    #[func]
    fn editor_build_acceptance_house(&self) -> bool {
        let mut guard = self
            .editor
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if guard.is_none() {
            return false;
        }
        let mut state = EditorState::new(room_demo::acceptance_house());
        let dims = state.volume.config().voxels();
        state.props = room_demo::acceptance_props()
            .into_iter()
            .filter_map(|prop| {
                let cell = [
                    (prop.position[0] / VOXEL_SIZE_M).floor(),
                    (prop.position[1] / VOXEL_SIZE_M).floor(),
                    (prop.position[2] / VOXEL_SIZE_M).floor(),
                ];
                if cell.iter().any(|value| *value < 0.0) {
                    return None;
                }
                let cell = [cell[0] as u32, cell[1] as u32, cell[2] as u32];
                if cell[0] as usize >= dims[0]
                    || cell[1] as usize >= dims[1]
                    || cell[2] as usize >= dims[2]
                {
                    return None;
                }
                Some(EditorProp {
                    kind: prop.kind,
                    cell,
                    rotation: 0.0,
                })
            })
            .collect();
        *guard = Some(state);
        true
    }

    /// Remove every allocated chunk from the editor volume.
    ///
    /// Returns `false` when no editor volume exists. The volume keeps its
    /// configured dimensions, so it can be filled again immediately.
    ///
    /// **Main thread only.**
    #[func]
    fn editor_clear(&self) -> bool {
        let mut guard = self
            .editor
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(state) = guard.as_mut() else {
            return false;
        };
        state.volume.clear();
        state.props.clear();
        state.mark_dirty();
        true
    }

    /// Read one 10 cm voxel from the editor volume.
    ///
    /// Returns the material id, or `-1` when there is no volume, the
    /// coordinate is negative or outside the volume, or the coordinate does
    /// not fit a volume position.
    ///
    /// **Main thread only.**
    #[func]
    fn editor_get_voxel(&self, x: i64, y: i64, z: i64) -> i64 {
        let Some(pos) = volume_pos_from_args(x, y, z) else {
            return -1;
        };
        let guard = self
            .editor
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(state) = guard.as_ref() else {
            return -1;
        };
        state.volume.get_voxel(pos).map_or(-1, i64::from)
    }

    /// Mesh the whole editor volume into one `ArrayMesh` surface.
    ///
    /// Uses packed arrays (`VERTEX`, `NORMAL`, `TEX_UV`, `INDEX`) in one
    /// neutral-material surface. Per-vertex material ids are intentionally not
    /// uploaded; multi-material rendering comes later. UVs are in meters, so a
    /// textured material must repeat.
    ///
    /// **Synchronous and main-thread only.** Whole-volume greedy meshing is a
    /// correctness-first path: the two-story benchmark volume is ~75 ms on a
    /// desktop CPU, and larger volumes are much slower. The editor must
    /// debounce edits or move remeshing to a worker before calling this per
    /// frame. An empty or missing volume returns an empty `ArrayMesh`.
    #[func]
    fn build_editor_volume_mesh(&self) -> Gd<ArrayMesh> {
        let mut guard = self
            .editor
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(state) = guard.as_mut() else {
            return ArrayMesh::new_gd();
        };

        let started = Instant::now();
        let buffers = state.volume.mesh();
        let mesh = match voxel_mesh_to_godot(&buffers) {
            Ok(mesh) => mesh,
            Err(message) => {
                eprintln!("Townsmith: {message}");
                return ArrayMesh::new_gd();
            }
        };
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;

        state.mesh_builds = state.mesh_builds.saturating_add(1);
        state.mesh_dirty = false;
        state.last_mesh_quads = buffers.quad_count();
        state.last_mesh_vertices = buffers.vertices.len();
        state.last_mesh_indices = buffers.indices.len();
        state.mesh_ms = elapsed_ms;
        mesh
    }

    /// Current editor volume and mesh statistics.
    ///
    /// Returns `created` (`bool`), the configuration keys from
    /// [`Self::editor_volume_size_voxels`], `solid_voxels`, `allocated_chunks`,
    /// `chunk_capacity`, `mesh_dirty`, `mesh_builds`, `last_mesh_quads`,
    /// `last_mesh_vertices`, `last_mesh_indices`, and `mesh_ms` (core meshing
    /// plus packed-array conversion for the last build). Without a volume it
    /// returns `{ "created": false, "error": "no_editor_volume" }`.
    ///
    /// **Main thread only.**
    #[func]
    fn editor_volume_stats(&self) -> VarDictionary {
        let mut result = VarDictionary::new();
        let guard = self
            .editor
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(state) = guard.as_ref() else {
            result.set("created", false);
            result.set("error", "no editor volume is open");
            return result;
        };

        result.set("created", true);
        volume_config_to_dictionary(state.volume.config(), &mut result);
        let (materials, _) = count_materials(&state.volume);
        result.set("solid_voxels", state.volume.solid_count() as i64);
        result.set("material_cost", slice_material_cost(&materials));
        result.set("allocated_chunks", state.volume.chunks_allocated() as i64);
        result.set("chunk_capacity", MAX_CHUNKS as i64);
        result.set("mesh_dirty", state.mesh_dirty);
        result.set("mesh_builds", state.mesh_builds as i64);
        result.set("last_mesh_quads", state.last_mesh_quads as i64);
        result.set("last_mesh_vertices", state.last_mesh_vertices as i64);
        result.set("last_mesh_indices", state.last_mesh_indices as i64);
        result.set("mesh_ms", state.mesh_ms);
        result
    }

    /// Place a functional prop in an empty editor voxel cell.
    ///
    /// `kind` is `0` bed, `1` stove, `2` toilet, `3` shower, `4` sink, `5`
    /// table, `6` sofa, `7` desk, `8` other. Replaces any prop already at the
    /// same cell. Props do not change the voxel mesh.
    ///
    /// **Main thread only.**
    #[func]
    fn editor_place_prop(&self, kind: i64, x: i64, y: i64, z: i64, rotation: f64) -> bool {
        let Some(kind) = prop_kind_from_code(kind) else {
            return false;
        };
        let Some(pos) = volume_pos_from_args(x, y, z) else {
            return false;
        };
        let mut guard = self
            .editor
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(state) = guard.as_mut() else {
            return false;
        };
        if state.volume.get_voxel(pos).ok() != Some(AIR) {
            return false;
        }
        if !rotation.is_finite() {
            return false;
        }
        let cell = [pos.x, pos.y, pos.z];
        state.props.retain(|prop| prop.cell != cell);
        state.props.push(EditorProp {
            kind,
            cell,
            rotation: (rotation as f32).rem_euclid(360.0),
        });
        true
    }

    /// Remove a prop at an exact editor voxel cell.
    ///
    /// Returns whether a prop was removed.
    ///
    /// **Main thread only.**
    #[func]
    fn editor_remove_prop_at(&self, x: i64, y: i64, z: i64) -> bool {
        let Some(pos) = volume_pos_from_args(x, y, z) else {
            return false;
        };
        let mut guard = self
            .editor
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(state) = guard.as_mut() else {
            return false;
        };
        let before = state.props.len();
        state
            .props
            .retain(|prop| prop.cell != [pos.x, pos.y, pos.z]);
        state.props.len() != before
    }

    /// List placed props as `{ kind, id, x, y, z, rotation }` dictionaries.
    ///
    /// **Main thread only.**
    #[func]
    fn editor_props(&self) -> VarDictionary {
        let mut result = VarDictionary::new();
        let guard = self
            .editor
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(state) = guard.as_ref() else {
            result.set("ok", false);
            result.set("error", "no editor volume is open");
            return result;
        };
        let mut props: Array<VarDictionary> = Array::new();
        for prop in &state.props {
            let mut item = VarDictionary::new();
            item.set("kind", prop_code(prop.kind));
            item.set("id", prop_id(prop.kind));
            item.set("x", prop.cell[0] as i64);
            item.set("y", prop.cell[1] as i64);
            item.set("z", prop.cell[2] as i64);
            item.set("rotation", f64::from(prop.rotation));
            props.push(&item);
        }
        result.set("ok", true);
        result.set("props", &props.to_variant());
        result
    }

    /// Remove all props without touching voxels.
    ///
    /// **Main thread only.**
    #[func]
    fn editor_clear_props(&self) -> bool {
        let mut guard = self
            .editor
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(state) = guard.as_mut() else {
            return false;
        };
        state.props.clear();
        true
    }

    /// Detect rooms and stats for the current editor volume.
    ///
    /// Returns `{ rooms, resident_capacity, job_slots, class, solid_voxels,
    /// materials }`. Room detection v0 is described in `docs/31`.
    ///
    /// **Main thread only.**
    #[func]
    fn editor_room_stats(&self) -> VarDictionary {
        let guard = self
            .editor
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(state) = guard.as_ref() else {
            let mut result = VarDictionary::new();
            result.set("ok", false);
            result.set("error", "no editor volume is open");
            return result;
        };
        let inputs = editor_props_input(&state.props);
        let mut result = room_stats_to_dictionary(&detect_rooms(
            &state.volume,
            &editor_room_materials(),
            &inputs,
        ));
        result.set("ok", true);
        result
    }

    /// Save the current editor volume as a `.tsbp` package.
    ///
    /// Returns `{ ok, path, hash, solid_voxels, bounds, rooms, props }` on success and
    /// `{ ok: false, error }` on failure. The volume is cloned before the file
    /// write so the editor lock is not held during I/O.
    ///
    /// **Main thread only.**
    #[func]
    fn editor_save_blueprint(&self, path: GString, id: GString, name: GString) -> VarDictionary {
        let mut result = VarDictionary::new();
        let path = match blueprint_path_from_arg(&path.to_string()) {
            Ok(path) => path,
            Err(error) => {
                result.set("ok", false);
                result.set("error", error);
                return result;
            }
        };
        let id = id.to_string();
        let name = name.to_string();
        if id.trim().is_empty() {
            result.set("ok", false);
            result.set("error", "blueprint id is empty");
            return result;
        }
        if name.trim().is_empty() {
            result.set("ok", false);
            result.set("error", "blueprint name is empty");
            return result;
        }
        if id.len() > 200 || name.len() > 200 {
            result.set("ok", false);
            result.set("error", "blueprint id and name must be at most 200 bytes");
            return result;
        }

        let (volume, props) = {
            let guard = self
                .editor
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let Some(state) = guard.as_ref() else {
                result.set("ok", false);
                result.set("error", "no editor volume is open");
                return result;
            };
            (state.volume.clone(), state.props.clone())
        };

        let blueprint = blueprint_from_editor_volume(id, name, volume, &props);
        let hash = match save_tsbp_with_hash(&path, &blueprint) {
            Ok(hash) => hash,
            Err(error) => {
                result.set("ok", false);
                result.set("error", error.to_string());
                return result;
            }
        };

        result.set("ok", true);
        result.set("path", path.to_string_lossy().to_string());
        result.set("hash", hash);
        result.set("solid_voxels", blueprint.metadata.solid_voxels as i64);
        result.set("material_cost", blueprint.metadata.stats.material_cost);
        result.set(
            "bounds",
            &PackedFloat32Array::from(blueprint.metadata.bounds_m.to_vec()).to_variant(),
        );
        result.set("rooms", blueprint.metadata.rooms.len() as i64);
        result.set("props", props.len() as i64);
        result
    }

    /// Load a `.tsbp` package into the editor, replacing the current volume.
    ///
    /// Returns `{ ok, path, hash, solid_voxels, bounds, rooms, props, name, id }` on
    /// success and `{ ok: false, error }` on failure.
    ///
    /// **Main thread only.**
    #[func]
    fn editor_load_blueprint(&self, path: GString) -> VarDictionary {
        let mut result = VarDictionary::new();
        let path = match blueprint_path_from_arg(&path.to_string()) {
            Ok(path) => path,
            Err(error) => {
                result.set("ok", false);
                result.set("error", error);
                return result;
            }
        };

        let blueprint = match load_tsbp(&path) {
            Ok(blueprint) => blueprint,
            Err(error) => {
                result.set("ok", false);
                result.set("error", error.to_string());
                return result;
            }
        };

        let hash = blueprint.metadata.content_hash.clone();
        let solid_voxels = blueprint.metadata.solid_voxels;
        let (materials, _) = count_materials(&blueprint.volume);
        let computed_cost = slice_material_cost(&materials);
        // Prefer a plausible stored value so save/load proves the metadata
        // roundtrip. Older packages without the field, or corrupt values, use
        // the current price table. Gameplay must still re-derive cost.
        let stored_cost = blueprint.metadata.stats.material_cost;
        let material_cost = if stored_cost.is_finite() && stored_cost > 0.0 {
            stored_cost
        } else {
            computed_cost
        };
        let bounds = PackedFloat32Array::from(blueprint.metadata.bounds_m.to_vec());
        let rooms = blueprint.metadata.rooms.len() as i64;
        let name = blueprint.metadata.name.clone();
        let id = blueprint.metadata.id.clone();

        let mut state = EditorState::new(blueprint.volume);
        let dims = state.volume.config().voxels();
        state.props = blueprint
            .metadata
            .props
            .iter()
            .filter_map(|prop| {
                let kind = prop_kind_from_id(&prop.id)?;
                let x = (prop.position[0] / VOXEL_SIZE_M).floor();
                let y = (prop.position[1] / VOXEL_SIZE_M).floor();
                let z = (prop.position[2] / VOXEL_SIZE_M).floor();
                if x < 0.0 || y < 0.0 || z < 0.0 {
                    return None;
                }
                let cell = [x as u32, y as u32, z as u32];
                if cell[0] as usize >= dims[0]
                    || cell[1] as usize >= dims[1]
                    || cell[2] as usize >= dims[2]
                {
                    return None;
                }
                Some(EditorProp {
                    kind,
                    cell,
                    rotation: prop.rotation,
                })
            })
            .collect();
        let mut seen = std::collections::HashSet::new();
        state.props.retain(|prop| seen.insert(prop.cell));
        let loaded_props = state.props.len();
        let mut guard = self
            .editor
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *guard = Some(state);

        result.set("ok", true);
        result.set("path", path.to_string_lossy().to_string());
        result.set("hash", hash);
        result.set("solid_voxels", solid_voxels as i64);
        result.set("material_cost", material_cost);
        result.set("bounds", &bounds.to_variant());
        result.set("rooms", rooms);
        result.set("props", loaded_props as i64);
        result.set("name", name);
        result.set("id", id);
        result
    }

    /// Load a `.tsbp` package and return its mesh without touching the editor.
    ///
    /// Used by the slice acceptance scene to place a blueprint **copy** next
    /// to the edited volume: the package is validated and hashed by
    /// [`load_tsbp`], then meshed exactly like the editor volume. Returns an
    /// empty `ArrayMesh` on an invalid path or package; call
    /// [`Self::editor_load_blueprint`] first when a readable error is needed.
    ///
    /// **Synchronous and main-thread only.** Whole-volume greedy meshing is
    /// ~75 ms for the acceptance house; call once per placed copy at load.
    #[func]
    fn build_blueprint_mesh(&self, path: GString) -> Gd<ArrayMesh> {
        let path = match blueprint_path_from_arg(&path.to_string()) {
            Ok(path) => path,
            Err(error) => {
                eprintln!("Townsmith: blueprint mesh rejected: {error}");
                return ArrayMesh::new_gd();
            }
        };
        let blueprint = match load_tsbp(&path) {
            Ok(blueprint) => blueprint,
            Err(error) => {
                eprintln!("Townsmith: blueprint mesh load failed: {error}");
                return ArrayMesh::new_gd();
            }
        };
        match voxel_mesh_to_godot(&blueprint.volume.mesh()) {
            Ok(mesh) => mesh,
            Err(message) => {
                eprintln!("Townsmith: {message}");
                ArrayMesh::new_gd()
            }
        }
    }
}

fn lot_request_from_args(
    seed: i64,
    min_x: i64,
    min_z: i64,
    max_x: i64,
    max_z: i64,
    zone: &GString,
) -> Result<(u64, LotRequest), VarDictionary> {
    let seed = u64::try_from(seed)
        .map_err(|_| placement_simple_error("invalid_seed", "lot seed must be non-negative"))?;
    let mut bounds = [0i32; 4];
    for (target, value) in bounds.iter_mut().zip([min_x, min_z, max_x, max_z]) {
        *target = i32::try_from(value).map_err(|_| {
            placement_simple_error("coordinate_out_of_range", "lot coordinate is outside i32")
        })?;
    }
    let zone = Zone::parse(&zone.to_string()).ok_or_else(|| {
        placement_simple_error(
            "invalid_zone",
            "zone must be residential, commercial, or industrial",
        )
    })?;
    Ok((seed, LotRequest { bounds, zone }))
}

fn lot_validation_result(request: &LotRequest) -> VarDictionary {
    let width = request.bounds[2] - request.bounds[0];
    let depth = request.bounds[3] - request.bounds[1];
    let mut result = VarDictionary::new();
    result.set("ok", true);
    result.set(
        "bounds",
        &PackedInt32Array::from(request.bounds.to_vec()).to_variant(),
    );
    result.set("zone", &GString::from(request.zone.as_str()));
    result.set("width_m", i64::from(width));
    result.set("depth_m", i64::from(depth));
    result.set("area_m2", i64::from(width * depth));
    result
}

fn lot_error_result(error: &LotError) -> VarDictionary {
    placement_simple_error(error.code(), error.to_string())
}

fn lot_to_dictionary(lot: &Lot) -> VarDictionary {
    let mut result = VarDictionary::new();
    result.set("id", lot.id as i64);
    result.set(
        "bounds",
        &PackedInt32Array::from(lot.bounds.to_vec()).to_variant(),
    );
    result.set("zone", &GString::from(lot.zone.as_str()));
    result.set("width_m", i64::from(lot.width_m()));
    result.set("depth_m", i64::from(lot.depth_m()));
    result.set("area_m2", i64::from(lot.area_m2()));
    result.set(
        "center",
        &PackedFloat32Array::from(lot.center().to_vec()).to_variant(),
    );
    result
}

fn settlement_error_result(error: &SettlementError) -> VarDictionary {
    placement_simple_error(error.code(), error.to_string())
}

fn settlement_to_dictionary(settlement: &Settlement) -> VarDictionary {
    let mut result = VarDictionary::new();
    result.set("id", settlement.id as i64);
    result.set("name", &GString::from(settlement.name.as_str()));
    result.set("owner", &GString::from(settlement.owner.as_str()));
    result.set(
        "center",
        &PackedInt32Array::from(settlement.center.to_vec()).to_variant(),
    );
    result.set("radius_m", i64::from(settlement.radius_m));
    result
}

fn road_request_from_args(
    x1: i64,
    z1: i64,
    x2: i64,
    z2: i64,
    width: i64,
) -> Result<RoadRequest, VarDictionary> {
    let Ok(start_x) = i32::try_from(x1) else {
        return Err(placement_simple_error(
            "coordinate_out_of_range",
            "road start X is out of range",
        ));
    };
    let Ok(start_z) = i32::try_from(z1) else {
        return Err(placement_simple_error(
            "coordinate_out_of_range",
            "road start Z is out of range",
        ));
    };
    let Ok(end_x) = i32::try_from(x2) else {
        return Err(placement_simple_error(
            "coordinate_out_of_range",
            "road end X is out of range",
        ));
    };
    let Ok(end_z) = i32::try_from(z2) else {
        return Err(placement_simple_error(
            "coordinate_out_of_range",
            "road end Z is out of range",
        ));
    };
    let Ok(width) = u8::try_from(width) else {
        return Err(placement_simple_error(
            "invalid_width",
            "road width must be 2..=24 m",
        ));
    };
    Ok(RoadRequest {
        start: [start_x, start_z],
        end: [end_x, end_z],
        width_m: width,
    })
}

fn road_length(request: &RoadRequest) -> i64 {
    i64::from((request.end[0] - request.start[0]).abs())
        + i64::from((request.end[1] - request.start[1]).abs())
}

fn road_error_result(error: &RoadError) -> VarDictionary {
    placement_simple_error(error.code(), error.to_string())
}

fn road_to_dictionary(road: &RoadSegment) -> VarDictionary {
    let mut result = VarDictionary::new();
    result.set("id", road.id as i64);
    result.set(
        "start",
        &PackedInt32Array::from(road.start.to_vec()).to_variant(),
    );
    result.set(
        "end",
        &PackedInt32Array::from(road.end.to_vec()).to_variant(),
    );
    result.set("width_m", i64::from(road.width_m));
    result.set("length_m", road.length_m());
    result.set(
        "center",
        &PackedFloat32Array::from(road.center().to_vec()).to_variant(),
    );
    result.set("runs_along_x", road.runs_along_x());
    result
}

fn placement_simple_error(code: &str, message: impl Into<String>) -> VarDictionary {
    let message = message.into();
    let mut result = VarDictionary::new();
    result.set("ok", false);
    result.set("error_code", code);
    result.set("error", &GString::from(message.as_str()));
    result
}

fn blueprint_error_code(error: &BlueprintError) -> &'static str {
    match error {
        BlueprintError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
            "missing_blueprint"
        }
        BlueprintError::Io(_) => "blueprint_io_error",
        BlueprintError::HashMismatch { .. } => "blueprint_hash_mismatch",
        BlueprintError::UnsupportedVersion(_) => "unsupported_blueprint_version",
        BlueprintError::MissingDependency(_) => "missing_blueprint_dependency",
        _ => "invalid_blueprint",
    }
}

fn placement_error_result(error: &PlacementError) -> VarDictionary {
    placement_simple_error(error.code(), error.to_string())
}

fn placement_to_dictionary(placement: &Placement) -> VarDictionary {
    let mut result = VarDictionary::new();
    result.set("id", placement.id as i64);
    result.set(
        "blueprint_id",
        &GString::from(placement.blueprint_id.as_str()),
    );
    result.set(
        "blueprint_hash",
        &GString::from(placement.blueprint_hash.as_str()),
    );
    result.set(
        "blueprint_path",
        &GString::from(placement.blueprint_path.as_str()),
    );
    result.set("name", &GString::from(placement.name.as_str()));
    result.set(
        "center",
        &PackedFloat32Array::from(placement.center.to_vec()).to_variant(),
    );
    result.set("rotation_deg", i64::from(placement.rotation_deg));
    result.set(
        "footprint",
        &PackedFloat32Array::from(placement.footprint_m.to_vec()).to_variant(),
    );
    result.set(
        "bounds",
        &PackedFloat32Array::from(placement.bounds_m.to_vec()).to_variant(),
    );
    result.set("foundation_m", f64::from(placement.foundation_m));
    result.set("material_cost", placement.material_cost);
    result.set("solid_voxels", placement.solid_voxels as i64);
    result.set("resident_capacity", i64::from(placement.resident_capacity));
    result.set("job_slots", i64::from(placement.job_slots));
    result
}

fn construction_site_to_dictionary(site: &ConstructionSite) -> VarDictionary {
    let mut result = VarDictionary::new();
    result.set("id", site.placement_id as i64);
    result.set("phase", &GString::from(site.phase.code()));
    result.set("phase_label", &GString::from(site.phase.label()));
    result.set("phase_progress", f64::from(site.phase_progress));
    result.set("progress", f64::from(site.progress()));
    result.set("elapsed_s", f64::from(site.elapsed_s));
    result.set("total_s", f64::from(site.total_s));
    result.set("materials_required", site.materials_required as i64);
    result.set("materials_delivered", site.materials_delivered as i64);
    result.set("crew_required", i64::from(site.crew_required));
    result.set("crew_assigned", i64::from(site.crew_assigned));
    result.set("complete", site.is_complete());
    if let Some(reason) = &site.stall_reason {
        result.set("stall_reason", &GString::from(reason.as_str()));
    }
    result
}

fn world_placement_request(
    seed: i64,
    path: &str,
    x: f64,
    z: f64,
    rotation: i64,
) -> Result<(u64, PlacementRequest), VarDictionary> {
    let seed = u64::try_from(seed)
        .map_err(|_| placement_simple_error("invalid_seed", "world seed must be non-negative"))?;
    let rotation = normalize_rotation(rotation).map_err(|error| placement_error_result(&error))?;
    let path = blueprint_path_from_arg(path)
        .map_err(|error| placement_simple_error("invalid_blueprint_path", error))?;
    let blueprint = load_tsbp(&path).map_err(|error| {
        placement_simple_error(
            "blueprint_load_failed",
            format!("{}: {error}", path.display()),
        )
    })?;
    let request = placement_request_from_blueprint(&blueprint, &path, x as f32, z as f32, rotation);
    Ok((seed, request))
}

fn placement_request_from_blueprint(
    blueprint: &Blueprint,
    path: &Path,
    center_x: f32,
    center_z: f32,
    rotation_deg: u16,
) -> PlacementRequest {
    let (materials, _) = count_materials(&blueprint.volume);
    let computed_cost = slice_material_cost(&materials);
    let stored_cost = blueprint.metadata.stats.material_cost;
    let material_cost = if stored_cost.is_finite() && stored_cost > 0.0 {
        stored_cost
    } else {
        computed_cost
    };
    PlacementRequest {
        blueprint_id: blueprint.metadata.id.clone(),
        blueprint_hash: blueprint.metadata.content_hash.clone(),
        blueprint_path: path.to_string_lossy().to_string(),
        name: blueprint.metadata.name.clone(),
        bounds_m: blueprint.metadata.bounds_m,
        center_x,
        center_z,
        rotation_deg,
        material_cost,
        resident_capacity: blueprint.metadata.stats.resident_capacity,
        job_slots: blueprint.metadata.stats.job_slots,
        solid_voxels: blueprint.metadata.solid_voxels,
    }
}

/// Drop staffing entries that no longer point at a completed workplace and
/// cap each remaining entry by its job slots and the household worker total.
fn sanitize_staffing(
    runtime: &mut CityRuntime,
    placements: &PlacementStore,
    construction: &ConstructionStore,
) {
    let slots: BTreeMap<u64, u32> = completed_workplaces(placements, construction)
        .into_iter()
        .map(|workplace| (workplace.id, workplace.slots))
        .collect();
    let mut remaining = runtime.workers();
    runtime.staffing.retain(|id, staffed| {
        let capacity = slots.get(id).copied().unwrap_or(0);
        let keep = (*staffed)
            .min(capacity)
            .min(u32::try_from(remaining).unwrap_or(u32::MAX));
        *staffed = keep;
        remaining = remaining.saturating_sub(u64::from(keep));
        keep > 0
    });
}

fn completed_slots(
    placements: &PlacementStore,
    construction: &ConstructionStore,
) -> Vec<(u64, u32)> {
    placements
        .placements
        .iter()
        .filter(|placement| {
            placement.resident_capacity > 0
                && construction
                    .site(placement.id)
                    .is_some_and(|site| site.is_complete())
        })
        .map(|placement| (placement.id, placement.resident_capacity))
        .collect()
}

fn completed_homes(placements: &PlacementStore, construction: &ConstructionStore) -> Vec<Home> {
    placements
        .placements
        .iter()
        .filter(|placement| {
            placement.resident_capacity > 0
                && construction
                    .site(placement.id)
                    .is_some_and(|site| site.is_complete())
        })
        .map(|placement| Home {
            id: placement.id,
            capacity: placement.resident_capacity,
            center: [placement.center[0], placement.center[2]],
        })
        .collect()
}

fn completed_workplaces(
    placements: &PlacementStore,
    construction: &ConstructionStore,
) -> Vec<Workplace> {
    placements
        .placements
        .iter()
        .filter(|placement| {
            placement.job_slots > 0
                && construction
                    .site(placement.id)
                    .is_some_and(|site| site.is_complete())
        })
        .map(|placement| Workplace {
            id: placement.id,
            slots: placement.job_slots,
            center: [placement.center[0], placement.center[2]],
        })
        .collect()
}

fn attach_construction(json: &str, store: &ConstructionStore) -> Result<String, String> {
    let mut value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| error.to_string())?;
    value["construction"] = serde_json::to_value(store).map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
}

fn construction_from_world_json(
    json: &str,
) -> Result<Option<ConstructionStore>, (&'static str, String)> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| ("invalid_json", error.to_string()))?;
    let Some(raw) = value.get("construction") else {
        return Ok(None);
    };
    let store: ConstructionStore = serde_json::from_value(raw.clone())
        .map_err(|error| ("invalid_construction", error.to_string()))?;
    store
        .validate()
        .map_err(|error| ("invalid_construction", error.to_string()))?;
    Ok(Some(store))
}

fn attach_population(json: &str, runtime: &CityRuntime) -> Result<String, String> {
    let mut value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| error.to_string())?;
    value["population"] = serde_json::to_value(runtime).map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
}

fn population_from_world_json(json: &str) -> Result<Option<CityRuntime>, (&'static str, String)> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| ("invalid_json", error.to_string()))?;
    let Some(raw) = value.get("population") else {
        return Ok(None);
    };
    let runtime: CityRuntime = serde_json::from_value(raw.clone())
        .map_err(|error| ("invalid_population", error.to_string()))?;
    runtime
        .validate()
        .map_err(|error| ("invalid_population", error.to_string()))?;
    Ok(Some(runtime))
}

fn attach_roads(json: &str, store: &RoadStore) -> Result<String, String> {
    let mut value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| error.to_string())?;
    value["roads"] = serde_json::to_value(store).map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
}

fn roads_from_world_json(json: &str) -> Result<Option<RoadStore>, (&'static str, String)> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| ("invalid_json", error.to_string()))?;
    let Some(raw) = value.get("roads") else {
        return Ok(None);
    };
    let store: RoadStore = serde_json::from_value(raw.clone())
        .map_err(|error| ("invalid_roads", error.to_string()))?;
    store
        .validate_loaded()
        .map_err(|error| ("invalid_roads", error.to_string()))?;
    Ok(Some(store))
}

fn attach_lots(json: &str, store: &LotStore) -> Result<String, String> {
    let mut value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| error.to_string())?;
    value["lots"] = serde_json::to_value(store).map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
}

fn lots_from_world_json(json: &str) -> Result<Option<LotStore>, (&'static str, String)> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| ("invalid_json", error.to_string()))?;
    let Some(raw) = value.get("lots") else {
        return Ok(None);
    };
    let store: LotStore =
        serde_json::from_value(raw.clone()).map_err(|error| ("invalid_lots", error.to_string()))?;
    store
        .validate_loaded()
        .map_err(|error| ("invalid_lots", error.to_string()))?;
    Ok(Some(store))
}

fn attach_settlements(json: &str, store: &SettlementStore) -> Result<String, String> {
    let mut value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| error.to_string())?;
    value["settlements"] = serde_json::to_value(store).map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
}

fn settlements_from_world_json(
    json: &str,
) -> Result<Option<SettlementStore>, (&'static str, String)> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| ("invalid_json", error.to_string()))?;
    let Some(raw) = value.get("settlements") else {
        return Ok(None);
    };
    let store: SettlementStore = serde_json::from_value(raw.clone())
        .map_err(|error| ("invalid_settlements", error.to_string()))?;
    store
        .validate_loaded()
        .map_err(|error| ("invalid_settlements", error.to_string()))?;
    Ok(Some(store))
}

fn attach_economy(json: &str, economy: &Economy) -> Result<String, String> {
    let mut value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| error.to_string())?;
    value["economy"] = serde_json::to_value(economy).map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
}

fn economy_from_world_json(json: &str) -> Result<Option<Economy>, (&'static str, String)> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| ("invalid_json", error.to_string()))?;
    let Some(raw) = value.get("economy") else {
        return Ok(None);
    };
    let economy: Economy = serde_json::from_value(raw.clone())
        .map_err(|error| ("invalid_economy", error.to_string()))?;
    economy
        .validate()
        .map_err(|error| ("invalid_economy", error.to_string()))?;
    Ok(Some(economy))
}

fn attach_industry(json: &str, store: &IndustryStore) -> Result<String, String> {
    let mut value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| error.to_string())?;
    value["industry"] = serde_json::to_value(store).map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
}

fn industry_from_world_json(json: &str) -> Result<Option<IndustryStore>, (&'static str, String)> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| ("invalid_json", error.to_string()))?;
    let Some(raw) = value.get("industry") else {
        return Ok(None);
    };
    let store: IndustryStore = serde_json::from_value(raw.clone())
        .map_err(|error| ("invalid_industry", error.to_string()))?;
    store
        .validate()
        .map_err(|error| ("invalid_industry", error.to_string()))?;
    Ok(Some(store))
}

fn attach_terrain_deltas(json: &str, bytes: &[u8], checksum: u64) -> Result<String, String> {
    let mut value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| error.to_string())?;
    value["terrain_deltas"] = serde_json::Value::String(hex_encode(bytes));
    value["terrain_checksum"] = serde_json::Value::String(format!("{checksum:016x}"));
    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
}

fn terrain_deltas_from_world_json(json: &str) -> Result<DeltaStore, (&'static str, String)> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| ("invalid_json", error.to_string()))?;
    let Some(encoded) = value.get("terrain_deltas") else {
        if value.get("terrain_checksum").is_some() {
            return Err((
                "invalid_terrain_checksum",
                "terrain_checksum requires terrain_deltas".to_string(),
            ));
        }
        return Ok(DeltaStore::new());
    };
    let Some(encoded) = encoded.as_str() else {
        return Err((
            "invalid_terrain_deltas",
            "terrain_deltas must be a hex string".to_string(),
        ));
    };
    let Some(checksum) = value
        .get("terrain_checksum")
        .and_then(|value| value.as_str())
    else {
        return Err((
            "invalid_terrain_checksum",
            "terrain_deltas requires terrain_checksum".to_string(),
        ));
    };
    let checksum = u64::from_str_radix(checksum, 16).map_err(|error| {
        (
            "invalid_terrain_checksum",
            format!("terrain checksum is not hexadecimal: {error}"),
        )
    })?;
    let bytes = hex_decode(encoded).map_err(|error| ("invalid_terrain_deltas", error))?;
    let actual = DeltaStore::checksum_bytes(&bytes);
    if actual != checksum {
        return Err((
            "terrain_checksum_mismatch",
            format!("expected {checksum:016x} but the deltas are {actual:016x}"),
        ));
    }
    DeltaStore::decode(&bytes).map_err(|error| ("invalid_terrain_deltas", error.to_string()))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn hex_decode(encoded: &str) -> Result<Vec<u8>, String> {
    let bytes = encoded.as_bytes();
    if !bytes.len().is_multiple_of(2) || !encoded.is_ascii() {
        return Err("terrain delta hex string has an invalid length or character".to_string());
    }
    let mut decoded = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.as_chunks::<2>().0 {
        decoded.push((hex_value(pair[0])? << 4) | hex_value(pair[1])?);
    }
    Ok(decoded)
}

fn hex_value(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(format!("invalid hex character '{}'", char::from(byte))),
    }
}

fn world_save_path_from_arg(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("world save path is empty".to_string());
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err("world save path must be absolute".to_string());
    }
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("world save path must not contain '..'".to_string());
    }
    let extension_ok = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"));
    if !extension_ok {
        return Err("world save path must end with .json".to_string());
    }
    Ok(path)
}

/// Refuse absurdly large world saves before reading them into memory.
const MAX_WORLD_SAVE_BYTES: u64 = 64 * 1024 * 1024;

fn write_text_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let temp = path.with_extension("json.tmp");
    let mut file = std::fs::File::create(&temp)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    drop(file);
    if let Err(error) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    Ok(())
}

/// Validate a GDScript blueprint path.
fn blueprint_path_from_arg(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("blueprint path is empty".to_string());
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err("blueprint path must be absolute".to_string());
    }
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("blueprint path must not contain '..'".to_string());
    }
    let extension_ok = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("tsbp"));
    if !extension_ok {
        return Err("blueprint path must end with .tsbp".to_string());
    }
    Ok(path)
}

/// Parse the GDScript chunk arguments, treating `0, 0, 0` as the default
/// 5 × 3 × 5 volume.
fn editor_config_from_args(
    chunks_x: i64,
    chunks_y: i64,
    chunks_z: i64,
) -> Result<VolumeConfig, String> {
    if chunks_x == 0 && chunks_y == 0 && chunks_z == 0 {
        return Ok(VolumeConfig::default());
    }

    let axis = |name: &str, value: i64| -> Result<u32, String> {
        let count = u32::try_from(value).map_err(|_| {
            format!(
                "editor volume {name} chunk count {value} must be between 1 and {MAX_CHUNKS_PER_AXIS}"
            )
        })?;
        if count > MAX_CHUNKS_PER_AXIS {
            return Err(format!(
                "editor volume {name} chunk count {count} exceeds the maximum {MAX_CHUNKS_PER_AXIS}"
            ));
        }
        Ok(count)
    };

    let chunks_x = axis("x", chunks_x)?;
    let chunks_y = axis("y", chunks_y)?;
    let chunks_z = axis("z", chunks_z)?;
    VolumeConfig::new(chunks_x, chunks_y, chunks_z)
        .map_err(|error| format!("invalid editor volume: {error}"))
}

/// Convert GDScript integer coordinates to a volume position without wrapping.
fn volume_pos_from_args(x: i64, y: i64, z: i64) -> Option<VolumePos> {
    Some(VolumePos::new(
        u32::try_from(x).ok()?,
        u32::try_from(y).ok()?,
        u32::try_from(z).ok()?,
    ))
}

/// Convert a GDScript material code to the core's `u16` material id.
fn material_from_args(material: i64) -> Option<MaterialId> {
    u16::try_from(material).ok()
}

fn volume_config_to_dictionary(config: VolumeConfig, target: &mut VarDictionary) {
    let chunks: Vec<i32> = config.chunks().iter().map(|&value| value as i32).collect();
    let voxels: Vec<i32> = config.voxels().iter().map(|&value| value as i32).collect();

    target.set("chunks", &PackedInt32Array::from(chunks).to_variant());
    target.set("chunk_count", config.chunk_count() as i64);
    target.set("voxels", &PackedInt32Array::from(voxels).to_variant());
    target.set(
        "bounds_m",
        &PackedFloat32Array::from(config.bounds_m().to_vec()).to_variant(),
    );
    target.set("chunk_size", CHUNK_SIZE as i64);
    target.set("voxel_size_m", f64::from(VOXEL_SIZE_M));
}

/// Upload one engine-neutral voxel mesh as a single packed-array surface.
///
/// Vertex colors and per-vertex material ids are deliberately omitted: the
/// editor uses one material override for now. UVs are passed through in meters.
fn voxel_mesh_to_godot(buffers: &MeshBuffers) -> Result<Gd<ArrayMesh>, &'static str> {
    if buffers.vertices.is_empty() || buffers.indices.is_empty() {
        return Ok(ArrayMesh::new_gd());
    }

    let vertex_count = buffers.vertices.len();
    let mut vertices = Vec::with_capacity(vertex_count);
    let mut normals = Vec::with_capacity(vertex_count);
    let mut uvs = Vec::with_capacity(vertex_count);

    for vertex in &buffers.vertices {
        vertices.push(Vector3::new(
            vertex.position[0],
            vertex.position[1],
            vertex.position[2],
        ));
        normals.push(Vector3::new(
            vertex.normal[0],
            vertex.normal[1],
            vertex.normal[2],
        ));
        uvs.push(Vector2::new(vertex.uv[0], vertex.uv[1]));
    }

    let mut indices = Vec::with_capacity(buffers.indices.len());
    for &index in &buffers.indices {
        if index as usize >= vertex_count {
            return Err("editor volume mesh has an index outside its vertex buffer");
        }
        let Ok(index) = i32::try_from(index) else {
            return Err("editor volume mesh exceeds the 32-bit Godot index range");
        };
        indices.push(index);
    }

    let mut arrays = VarArray::new();
    arrays.resize(ArrayType::MAX.ord() as usize, &Variant::nil());
    arrays.set(
        ArrayType::VERTEX.ord() as usize,
        &PackedVector3Array::from(vertices),
    );
    arrays.set(
        ArrayType::NORMAL.ord() as usize,
        &PackedVector3Array::from(normals),
    );
    arrays.set(
        ArrayType::TEX_UV.ord() as usize,
        &PackedVector2Array::from(uvs),
    );
    arrays.set(
        ArrayType::INDEX.ord() as usize,
        &PackedInt32Array::from(indices),
    );

    let mut mesh = ArrayMesh::new_gd();
    mesh.add_surface_from_arrays(PrimitiveType::TRIANGLES, &arrays);
    Ok(mesh)
}

fn clamp_step(step: i64) -> usize {
    usize::try_from(step).map_or(1, |step| step.clamp(1, WORLD_CHUNK_SIZE))
}

fn terrain_mesh_to_godot(buffers: &TerrainMesh) -> Gd<ArrayMesh> {
    if buffers.indices.is_empty() {
        return ArrayMesh::new_gd();
    }

    let vertices: Vec<Vector3> = buffers
        .vertices
        .iter()
        .map(|vertex| Vector3::new(vertex.position[0], vertex.position[1], vertex.position[2]))
        .collect();
    let normals: Vec<Vector3> = buffers
        .vertices
        .iter()
        .map(|vertex| Vector3::new(vertex.normal[0], vertex.normal[1], vertex.normal[2]))
        .collect();
    let colors: Vec<Color> = buffers
        .vertices
        .iter()
        .map(|vertex| Color::from_rgb(vertex.color[0], vertex.color[1], vertex.color[2]))
        .collect();
    let indices: Vec<i32> = buffers.indices.iter().map(|&index| index as i32).collect();

    let mut arrays = VarArray::new();
    arrays.resize(ArrayType::MAX.ord() as usize, &Variant::nil());
    arrays.set(
        ArrayType::VERTEX.ord() as usize,
        &PackedVector3Array::from(vertices),
    );
    arrays.set(
        ArrayType::NORMAL.ord() as usize,
        &PackedVector3Array::from(normals),
    );
    arrays.set(
        ArrayType::COLOR.ord() as usize,
        &PackedColorArray::from(colors),
    );
    arrays.set(
        ArrayType::INDEX.ord() as usize,
        &PackedInt32Array::from(indices),
    );

    let mut mesh = ArrayMesh::new_gd();
    mesh.add_surface_from_arrays(PrimitiveType::TRIANGLES, &arrays);
    mesh
}

fn city_tier_from_code(code: i64) -> Option<CityTier> {
    match code {
        0 => Some(CityTier::Early),
        1 => Some(CityTier::Mid),
        2 => Some(CityTier::Late),
        _ => None,
    }
}

fn city_layout_to_dictionary(layout: &CityLayout) -> VarDictionary {
    let positions: Vec<Vector3> = layout
        .buildings
        .iter()
        .map(|building| {
            Vector3::new(
                building.position[0],
                building.position[1],
                building.position[2],
            )
        })
        .collect();
    let sizes: Vec<Vector3> = layout
        .buildings
        .iter()
        .map(|building| Vector3::new(building.size[0], building.size[1], building.size[2]))
        .collect();
    let colors: Vec<Color> = layout
        .buildings
        .iter()
        .map(|building| Color::from_rgb(building.color[0], building.color[1], building.color[2]))
        .collect();
    let kinds: Vec<i32> = layout
        .buildings
        .iter()
        .map(|building| building.kind as i32)
        .collect();

    let mut result = VarDictionary::new();
    result.set("tier", &GString::from(layout.tier.as_str()).to_variant());
    result.set("seed", layout.seed as i64);
    result.set("population", layout.population as i64);
    result.set("buildings", layout.building_count() as i64);
    result.set(
        "bounds",
        &PackedFloat32Array::from(layout.bounds_m.to_vec()).to_variant(),
    );
    result.set(
        "positions",
        &PackedVector3Array::from(positions).to_variant(),
    );
    result.set("sizes", &PackedVector3Array::from(sizes).to_variant());
    result.set("colors", &PackedColorArray::from(colors).to_variant());
    result.set("kinds", &PackedInt32Array::from(kinds).to_variant());
    let checksum = layout.checksum().to_string();
    result.set("checksum", &GString::from(&checksum).to_variant());
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait_for_result(bridge: &TerrainBridge, id: i64) -> TerrainResult {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match bridge.poll() {
                PollOutcome::Result(result) => return result,
                PollOutcome::Empty => {
                    assert!(
                        Instant::now() < deadline,
                        "timed out waiting for terrain result {id}"
                    );
                    thread::sleep(Duration::from_millis(5));
                }
                PollOutcome::Disconnected => panic!("terrain workers disconnected"),
            }
        }
    }

    #[test]
    fn terrain_mesh_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<TerrainMesh>();
    }

    #[test]
    fn request_round_trips_a_mesh() {
        let mut bridge = TerrainBridge::new();
        let id = bridge.request(2026, 0, 0, 1, Arc::new(RwLock::new(DeltaStore::new())));
        assert!(id > 0);

        let result = wait_for_result(&bridge, id);
        assert_eq!(result.id, id as u64);
        assert_eq!(result.chunk_x, 0);
        assert_eq!(result.chunk_z, 0);
        assert_eq!(result.step, 1);
        assert!(result.error.is_none());
        assert!(result.mesh.as_ref().is_some_and(|mesh| !mesh.is_empty()));
    }

    #[test]
    fn concurrent_requests_each_return_once() {
        let mut bridge = TerrainBridge::new();
        let ids: Vec<i64> = (0..4)
            .map(|index| {
                bridge.request(2026, index, 0, 1, Arc::new(RwLock::new(DeltaStore::new())))
            })
            .collect();
        assert!(ids.iter().all(|id| *id > 0));

        let mut seen = Vec::new();
        for _ in 0..ids.len() {
            let result = wait_for_result(&bridge, ids[0]);
            seen.push(result.id);
        }
        seen.sort_unstable();
        let mut expected: Vec<u64> = ids.iter().map(|id| *id as u64).collect();
        expected.sort_unstable();
        assert_eq!(seen, expected);
    }

    #[test]
    fn worker_matches_synchronous_skirted_mesh() {
        let generator = WorldGenerator::new(2026);
        let chunk = generator
            .generate(WorldChunkCoord::new(0, 0))
            .expect("generated chunk");
        let expected = terrain_mesh_lod_with_sampler_and_skirt(
            &chunk,
            2,
            |x, z| generator.height_at(TerrainPos::new(x, z)),
            DEFAULT_SKIRT_DEPTH_M,
        );
        let unskirted = core_world::terrain_mesh_lod_with_sampler(&chunk, 2, |x, z| {
            generator.height_at(TerrainPos::new(x, z))
        });
        assert_eq!(expected.quad_count(), unskirted.quad_count() + 4 * 128);

        let mut bridge = TerrainBridge::new();
        let id = bridge.request(2026, 0, 0, 2, Arc::new(RwLock::new(DeltaStore::new())));
        let result = wait_for_result(&bridge, id);
        assert!(result.error.is_none());
        assert_eq!(result.mesh.expect("mesh"), expected);
    }

    #[test]
    fn worker_applies_terrain_deltas() {
        let generator = WorldGenerator::new(2026);
        let coord = WorldChunkCoord::new(0, 0);
        let mut deltas = DeltaStore::new();
        deltas.set_height(coord, 10, 20, 200).expect("edit");
        let mut edited = generator.generate(coord).expect("generated chunk");
        deltas.apply_to(&mut edited);
        let expected = terrain_mesh_lod_with_sampler_and_skirt(
            &edited,
            1,
            |x, z| deltas.height_at(&generator, TerrainPos::new(x, z)),
            DEFAULT_SKIRT_DEPTH_M,
        );

        let mut bridge = TerrainBridge::new();
        let id = bridge.request(2026, 0, 0, 1, Arc::new(RwLock::new(deltas)));
        let result = wait_for_result(&bridge, id);
        assert!(result.error.is_none());
        assert_eq!(result.mesh.expect("mesh"), expected);
    }

    #[test]
    fn city_tier_codes_are_strict() {
        assert_eq!(city_tier_from_code(0), Some(CityTier::Early));
        assert_eq!(city_tier_from_code(1), Some(CityTier::Mid));
        assert_eq!(city_tier_from_code(2), Some(CityTier::Late));
        assert_eq!(city_tier_from_code(-1), None);
        assert_eq!(city_tier_from_code(3), None);
        assert_eq!(city_tier_from_code(99), None);
    }

    #[test]
    fn editor_config_accepts_default_and_mixed_axes() {
        let default = editor_config_from_args(0, 0, 0).expect("default config");
        assert_eq!(default.chunks(), [5, 3, 5]);

        let custom = editor_config_from_args(2, 4, 6).expect("custom config");
        assert_eq!(custom.chunks(), [2, 4, 6]);
    }

    #[test]
    fn editor_config_rejects_mixed_zero_axes() {
        assert!(editor_config_from_args(0, 3, 5).is_err());
        assert!(editor_config_from_args(5, 0, 5).is_err());
        assert!(editor_config_from_args(5, 3, 0).is_err());
    }

    #[test]
    fn editor_config_rejects_negative_and_oversized_axes() {
        assert!(editor_config_from_args(-1, 3, 5).is_err());
        assert!(editor_config_from_args(33, 3, 5).is_err());
        assert!(editor_config_from_args(32, 32, 32).is_err());
    }

    #[test]
    fn material_conversion_checks_the_u16_range() {
        assert_eq!(material_from_args(0), Some(0));
        assert_eq!(material_from_args(1), Some(1));
        assert_eq!(material_from_args(65_535), Some(65_535));
        assert_eq!(material_from_args(-1), None);
        assert_eq!(material_from_args(65_536), None);
    }

    #[test]
    fn slice_cost_prices_the_acceptance_house() {
        let volume = room_demo::acceptance_house();
        let (materials, solid) = count_materials(&volume);
        assert_eq!(solid, 64_224);
        assert!((slice_material_cost(&materials) - 38_028.8).abs() < 1e-6);
    }

    #[test]
    fn volume_position_rejects_negative_and_large_values() {
        assert_eq!(volume_pos_from_args(0, 0, 0), Some(VolumePos::new(0, 0, 0)));
        assert_eq!(volume_pos_from_args(-1, 0, 0), None);
        assert_eq!(volume_pos_from_args(0, i64::MAX, 0), None);
    }

    #[test]
    fn blueprint_path_requires_an_absolute_tsbp_path() {
        let abs = std::env::temp_dir().join("house.tsbp");
        let abs_upper = std::env::temp_dir().join("house.TSBP");
        let traversal = std::env::temp_dir().join("..").join("house.tsbp");
        assert!(blueprint_path_from_arg(&format!(" {} ", abs.display())).is_ok());
        assert!(blueprint_path_from_arg(&abs_upper.to_string_lossy()).is_ok());
        assert!(blueprint_path_from_arg("").is_err());
        assert!(blueprint_path_from_arg("   ").is_err());
        assert!(blueprint_path_from_arg("house.tsbp").is_err());
        assert!(blueprint_path_from_arg(&traversal.to_string_lossy()).is_err());
        assert!(
            blueprint_path_from_arg(&std::env::temp_dir().join("house.zip").to_string_lossy())
                .is_err()
        );
        assert!(
            blueprint_path_from_arg(&std::env::temp_dir().join("house").to_string_lossy()).is_err()
        );
    }

    #[test]
    fn world_save_path_requires_an_absolute_json_path() {
        let abs = std::env::temp_dir().join("world.json");
        let abs_upper = std::env::temp_dir().join("world.JSON");
        let traversal = std::env::temp_dir().join("..").join("world.json");
        assert!(world_save_path_from_arg(&format!(" {} ", abs.display())).is_ok());
        assert!(world_save_path_from_arg(&abs_upper.to_string_lossy()).is_ok());
        assert!(world_save_path_from_arg("").is_err());
        assert!(world_save_path_from_arg("world.json").is_err());
        assert!(world_save_path_from_arg(&traversal.to_string_lossy()).is_err());
        assert!(world_save_path_from_arg(
            &std::env::temp_dir().join("world.tsbp").to_string_lossy()
        )
        .is_err());
        assert!(
            world_save_path_from_arg(&std::env::temp_dir().join("world").to_string_lossy())
                .is_err()
        );
    }

    #[test]
    fn terrain_delta_hex_round_trips() {
        let bytes = [0, 1, 15, 16, 127, 128, 255];
        let encoded = hex_encode(&bytes);
        assert_eq!(encoded, "00010f107f80ff");
        assert_eq!(hex_decode(&encoded).expect("decode"), bytes);
        assert!(hex_decode("0").is_err());
        assert!(hex_decode("zz").is_err());
    }

    #[test]
    fn world_json_round_trips_terrain_deltas() {
        let store = PlacementStore::new(2026);
        let json = store.to_json_pretty().expect("placement json");
        let mut deltas = DeltaStore::new();
        deltas
            .set_height(WorldChunkCoord::new(0, 0), 169, 125, 42)
            .expect("edit");
        let encoded = attach_terrain_deltas(&json, &deltas.encode(), deltas.checksum())
            .expect("attach deltas");
        let decoded = terrain_deltas_from_world_json(&encoded).expect("read deltas");
        assert_eq!(decoded, deltas);
    }

    #[test]
    fn world_json_rejects_bad_terrain_checksum() {
        let store = PlacementStore::new(2026);
        let json = store.to_json_pretty().expect("placement json");
        let deltas = DeltaStore::new();
        let encoded = attach_terrain_deltas(&json, &deltas.encode(), deltas.checksum())
            .expect("attach deltas")
            .replace(&format!("{:016x}", deltas.checksum()), "0000000000000000");
        assert!(terrain_deltas_from_world_json(&encoded).is_err());

        let mut missing: serde_json::Value = serde_json::from_str(
            &attach_terrain_deltas(&json, &deltas.encode(), deltas.checksum())
                .expect("attach deltas"),
        )
        .expect("json");
        missing
            .as_object_mut()
            .expect("object")
            .remove("terrain_checksum");
        let missing = serde_json::to_string(&missing).expect("json");
        assert!(terrain_deltas_from_world_json(&missing).is_err());
    }

    #[test]
    fn old_world_json_has_empty_terrain_deltas() {
        let json = PlacementStore::new(2026)
            .to_json_pretty()
            .expect("placement json");
        let deltas = terrain_deltas_from_world_json(&json).expect("read deltas");
        assert!(deltas.is_empty());
    }

    #[test]
    fn construction_json_round_trips_through_world_save() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        store.add_site(7, 64_224).expect("site");
        store.tick(10.0);
        let json = attach_construction("{}", &store).expect("attach");
        let decoded = construction_from_world_json(&json)
            .expect("parse")
            .expect("construction field");
        assert_eq!(decoded, store);
        assert_eq!(decoded.checksum(), store.checksum());
    }

    #[test]
    fn construction_json_rejects_unknown_version() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        store.add_site(7, 10_000).expect("site");
        store.format_version = 99;
        let json = attach_construction("{}", &store).expect("attach");
        assert!(construction_from_world_json(&json).is_err());
    }

    #[test]
    fn road_json_round_trips_through_world_save() {
        let mut store = RoadStore::new(2026);
        let request = RoadRequest {
            start: [180, 125],
            end: [200, 125],
            width_m: 8,
        };
        store.add(&request, |_, _| 40).expect("road");
        let json = attach_roads("{}", &store).expect("attach");
        let decoded = roads_from_world_json(&json)
            .expect("parse")
            .expect("roads field");
        assert_eq!(decoded, store);
        assert_eq!(decoded.checksum(), store.checksum());
    }

    #[test]
    fn road_json_rejects_unknown_version() {
        let mut store = RoadStore::new(2026);
        store.format_version = 99;
        let json = attach_roads("{}", &store).expect("attach");
        assert!(roads_from_world_json(&json).is_err());
    }

    #[test]
    fn lot_json_round_trips_through_world_save() {
        let mut roads = RoadStore::new(2026);
        roads
            .add(
                &RoadRequest {
                    start: [180, 125],
                    end: [200, 125],
                    width_m: 8,
                },
                |_, _| 40,
            )
            .expect("road");
        let mut store = LotStore::new(2026);
        store
            .add(
                &LotRequest {
                    bounds: [182, 128, 198, 144],
                    zone: Zone::Residential,
                },
                |_, _| 40,
                &roads,
            )
            .expect("lot");
        let json = attach_lots("{}", &store).expect("attach");
        let decoded = lots_from_world_json(&json)
            .expect("parse")
            .expect("lots field");
        assert_eq!(decoded, store);
        assert_eq!(decoded.checksum(), store.checksum());
    }

    #[test]
    fn lot_json_rejects_unknown_version() {
        let mut store = LotStore::new(2026);
        store.format_version = 99;
        let json = attach_lots("{}", &store).expect("attach");
        assert!(lots_from_world_json(&json).is_err());
    }

    #[test]
    fn settlement_world_json_round_trips_and_old_saves_are_closed() {
        let mut store = SettlementStore::new(2026);
        store
            .found(&SettlementRequest {
                name: "Riverbend".to_string(),
                owner: "player".to_string(),
                center: [10, 20],
                radius_m: 64,
            })
            .expect("settlement");
        let json = attach_settlements("{}", &store).expect("attach");
        let decoded = settlements_from_world_json(&json)
            .expect("parse")
            .expect("present");
        assert_eq!(decoded, store);
        assert!(settlements_from_world_json("{}")
            .expect("old save")
            .is_none());
    }

    #[test]
    fn economy_world_json_round_trips_and_old_saves_are_closed() {
        let mut economy = Economy::new(2026);
        economy.tick(3600.0, 50, 40, 10);
        let json = attach_economy("{}", &economy).expect("attach");
        let decoded = economy_from_world_json(&json)
            .expect("parse")
            .expect("present");
        assert!((decoded.balance - economy.balance).abs() < 0.001);
        assert!(economy_from_world_json("{}").expect("old save").is_none());
    }

    #[test]
    fn economy_world_json_rejects_invalid_balance() {
        let mut economy = Economy::new(2026);
        economy.balance = f64::NAN;
        let json = serde_json::to_string(&serde_json::json!({"economy": economy})).expect("json");
        let error = economy_from_world_json(&json).expect_err("reject");
        assert_eq!(error.0, "invalid_economy");
    }

    #[test]
    fn industry_world_json_round_trips_and_old_saves_are_closed() {
        let mut store = IndustryStore::new(2026);
        store.add_site(1, IndustryKind::Farm).expect("site");
        store.add_site(2, IndustryKind::Hunt).expect("site");
        store.tick(core_city::DAY_SECONDS, &[(1, 10)].into_iter().collect(), 20);
        let json = attach_industry("{}", &store).expect("attach");
        let decoded = industry_from_world_json(&json)
            .expect("parse")
            .expect("present");
        assert_eq!(decoded, store);
        assert!(industry_from_world_json("{}").expect("old save").is_none());
    }

    #[test]
    fn industry_world_json_rejects_unknown_kind_and_invalid_stock() {
        let mut store = IndustryStore::new(2026);
        store.add_site(1, IndustryKind::Farm).expect("site");
        let json = attach_industry("{}", &store).expect("attach");
        let mut value: serde_json::Value = serde_json::from_str(&json).expect("value");
        value["industry"]["sites"]["1"]["kind"] = serde_json::json!("mine");
        let json = serde_json::to_string(&value).expect("json");
        assert_eq!(
            industry_from_world_json(&json).expect_err("reject").0,
            "invalid_industry"
        );

        let mut store = IndustryStore::new(2026);
        store.add_site(1, IndustryKind::Farm).expect("site");
        store.stockpile.food = -1.0;
        let json = serde_json::to_string(&serde_json::json!({"industry": store})).expect("json");
        assert_eq!(
            industry_from_world_json(&json).expect_err("reject").0,
            "invalid_industry"
        );
    }

    #[test]
    fn population_staffing_round_trips_in_world_json() {
        let mut runtime = CityRuntime::new(2026);
        runtime.set_move_in_per_second(2.0);
        runtime.tick(1.0, 1, &[(1, 1)]);
        let json = attach_population("{}", &runtime).expect("attach");
        let decoded = population_from_world_json(&json)
            .expect("parse")
            .expect("present");
        assert_eq!(decoded.staffing, runtime.staffing);
    }

    #[test]
    fn sanitize_staffing_drops_unknown_workplaces() {
        let mut runtime = CityRuntime::new(2026);
        runtime.staffing.insert(99, 5);
        sanitize_staffing(
            &mut runtime,
            &PlacementStore::new(2026),
            &ConstructionStore::new(4, 1_000_000),
        );
        assert!(runtime.staffing.is_empty());
    }

    #[test]
    fn settlement_world_json_rejects_bad_next_id() {
        let mut store = SettlementStore::new(2026);
        store
            .found(&SettlementRequest {
                name: "Riverbend".to_string(),
                owner: "player".to_string(),
                center: [10, 20],
                radius_m: 64,
            })
            .expect("settlement");
        let mut value = serde_json::to_value(&store).expect("value");
        value["next_id"] = serde_json::json!(1);
        let json = serde_json::to_string(&value).expect("json");
        let error = settlements_from_world_json(&json).expect_err("reject");
        assert_eq!(error.0, "invalid_settlements");
    }

    #[test]
    fn blueprint_metadata_includes_detected_rooms() {
        let mut volume =
            VoxelVolume::new(VolumeConfig::new(2, 1, 2).expect("config")).expect("volume");
        volume
            .fill_box(
                VolumePos::new(0, 0, 0),
                VolumePos::new(40, 1, 40),
                EDITOR_FLOOR,
            )
            .expect("floor");
        volume
            .fill_box(
                VolumePos::new(0, 30, 0),
                VolumePos::new(40, 31, 40),
                EDITOR_CEILING,
            )
            .expect("ceiling");
        volume
            .fill_box(
                VolumePos::new(0, 1, 0),
                VolumePos::new(1, 30, 40),
                EDITOR_WALL,
            )
            .expect("west wall");
        volume
            .fill_box(
                VolumePos::new(39, 1, 0),
                VolumePos::new(40, 30, 40),
                EDITOR_WALL,
            )
            .expect("east wall");
        volume
            .fill_box(
                VolumePos::new(1, 1, 0),
                VolumePos::new(39, 30, 1),
                EDITOR_WALL,
            )
            .expect("north wall");
        volume
            .fill_box(
                VolumePos::new(1, 1, 39),
                VolumePos::new(39, 30, 40),
                EDITOR_WALL,
            )
            .expect("south wall");
        volume
            .set_voxel(VolumePos::new(0, 15, 20), EDITOR_DOOR)
            .expect("door");
        volume
            .set_voxel(VolumePos::new(0, 15, 10), EDITOR_WINDOW)
            .expect("window");

        let solid = u64::from(volume.solid_count());
        let props = vec![EditorProp {
            kind: PropKind::Bed,
            cell: [10, 1, 10],
            rotation: 0.0,
        }];
        let blueprint = blueprint_from_editor_volume(
            "local:test_room".to_string(),
            "Test Room".to_string(),
            volume,
            &props,
        );
        assert_eq!(blueprint.metadata.rooms.len(), 1);
        let room = &blueprint.metadata.rooms[0];
        assert_eq!(room.id, "room_0");
        assert_eq!(room.room_type, "bedroom");
        assert_eq!(room.floor, 0);
        assert!((room.area_m2 - 14.44).abs() < 0.001);
        assert!((room.volume_m3 - 43.32).abs() < 0.001);
        assert_eq!(room.beds, 1);
        assert_eq!(room.doors, 1);
        assert_eq!(room.windows, 1);
        assert_eq!(blueprint.metadata.solid_voxels, solid);
        assert_eq!(blueprint.metadata.bounds_m, [6.4, 3.2, 6.4]);
        assert_eq!(blueprint.metadata.props.len(), 1);
        assert_eq!(blueprint.metadata.props[0].id, "base:bed_single");
        let position = blueprint.metadata.props[0].position;
        assert!((position[0] - 1.05).abs() < 1e-6);
        assert!((position[1] - 0.1).abs() < 1e-6);
        assert!((position[2] - 1.05).abs() < 1e-6);
        assert_eq!(
            blueprint.metadata.materials,
            [
                ("material:1".to_string(), 4522),
                ("material:2".to_string(), 1600),
                ("material:3".to_string(), 1600),
                ("material:4".to_string(), 1),
                ("material:5".to_string(), 1),
            ]
            .into_iter()
            .collect()
        );
        assert_eq!(blueprint.metadata.stats.resident_capacity, 1);
        assert_eq!(blueprint.metadata.stats.job_slots, 0);
        assert_eq!(blueprint.metadata.stats.class, "middle");

        let path = std::env::temp_dir().join(format!(
            "townsmith_room_metadata_{}.tsbp",
            std::process::id()
        ));
        let hash = save_tsbp_with_hash(&path, &blueprint).expect("save");
        let loaded = load_tsbp(&path).expect("load");
        let _ = std::fs::remove_file(path);
        assert_eq!(loaded.metadata.content_hash, hash);
        assert_eq!(loaded.metadata.rooms, blueprint.metadata.rooms);
        assert_eq!(loaded.metadata.materials, blueprint.metadata.materials);
        assert_eq!(loaded.metadata.stats, blueprint.metadata.stats);
        assert_eq!(loaded.metadata.props, blueprint.metadata.props);
    }

    #[test]
    fn prop_ids_round_trip() {
        for kind in [
            PropKind::Bed,
            PropKind::Stove,
            PropKind::Toilet,
            PropKind::Shower,
            PropKind::Sink,
            PropKind::Table,
            PropKind::Sofa,
            PropKind::Desk,
            PropKind::Other,
        ] {
            assert_eq!(prop_kind_from_id(prop_id(kind)), Some(kind));
        }
    }

    #[test]
    fn room_bounds_payload_maps_cells_to_meters() {
        let bounds = RoomBounds {
            min_x: 1,
            max_x: 38,
            min_z: 1,
            max_z: 38,
            floor: 0,
            band_height_voxels: 30,
        };
        let (bounds_m, center_m) = room_bounds_payload(&bounds, 0);
        assert_eq!(bounds_m, [0.1, 0.1, 3.9, 3.9]);
        assert_eq!(center_m, [2.0, 1.5, 2.0]);

        let partial = RoomBounds {
            floor: 3,
            band_height_voxels: 6,
            ..bounds
        };
        let (_, partial_center) = room_bounds_payload(&partial, 3);
        assert_eq!(partial_center[1], 9.3);
    }

    #[test]
    fn population_round_trips_in_world_json() {
        let mut runtime = CityRuntime::new(2026);
        runtime.tick(2.0, 2, &[(1, 2)]);
        let json = attach_population("{}", &runtime).expect("attach");
        let parsed = population_from_world_json(&json)
            .expect("parse")
            .expect("present");
        assert_eq!(parsed, runtime);
        assert!(population_from_world_json("{}")
            .expect("old save")
            .is_none());
    }

    #[test]
    fn population_rejects_an_unsupported_version() {
        let mut runtime = CityRuntime::new(2026);
        runtime.format_version = 99;
        let json = attach_population("{}", &runtime).expect("attach");
        assert!(population_from_world_json(&json).is_err());
    }

    #[test]
    fn drop_joins_workers_without_hanging() {
        let bridge = TerrainBridge::new();
        drop(bridge);
    }
}
