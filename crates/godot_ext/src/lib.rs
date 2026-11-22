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
use std::sync::{Arc, Mutex, OnceLock, RwLock};
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
use core_city::named::{age_years_from_days, slot_is_employed, MAX_AGE_DAYS};
use core_city::{
    advise, diff_selection, evaluate_tutorial, heatmap_grid, normalize_rotation, starter,
    summarize, summarize_district, summarize_heatmap, AdvisorInput, AdvisorReport, Age, AgeState,
    CitizenDeath, CitizenMotion, CitizenPosition, CityLayout, CityRuntime, CitySummary, CityTier,
    ConstructionPhase, ConstructionSite, ConstructionStore, ConstructionTimeScale, DistrictHeatmap,
    DistrictPolicy, DistrictPolicyError, DistrictPolicyRequest, DistrictPolicyStore,
    DistrictSummary, Economy, Home, Household, IndustryKind, IndustryStore, InfrastructureCosts,
    LifecycleTick, Lot, LotError, LotRequest, LotStore, MaterializationBudget,
    MaterializationSelection, MaterializedCitizen, MotionLeg, NamedCitizens, NamedSave, Placement,
    PlacementError, PlacementRequest, PlacementStore, ResearchStore, RoadError, RoadGraph,
    RoadRequest, RoadSegment, RoadStore, RouteCache, ServiceConsumer, ServiceKind, ServiceReport,
    ServiceStore, ServiceTier, Settlement, SettlementError, SettlementRequest, SettlementStore,
    TechTree, Trade, TradePool, TutorialInput, UtilityConsumer, UtilityKind, UtilityReport,
    UtilityStore, Workplace, Zone, DEFAULT_LOAN_RATE_PER_DAY, DEFAULT_LOAN_TERM_DAYS,
    DEFAULT_ROUTE_CACHE_CAPACITY, MAX_HEATMAP_CELLS, MAX_HEATMAP_CELL_M, MAX_LOANS, MAX_TOTAL_DEBT,
    MAX_WORLD_COORD_M, MIN_HEATMAP_CELL_M, RESEARCH_PER_RESIDENT_PER_DAY, STARTING_BALANCE,
};
use core_rooms::{
    count_materials, demo as room_demo, detect_rooms, PropInput, PropKind, RoomBounds,
    RoomMaterials, RoomStats, FLOOR_BAND_M,
};
use core_structure::{StructuralSystem, StructureSpec};
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
#[allow(clippy::too_many_arguments)]
fn blueprint_from_editor_volume(
    id: String,
    name: String,
    volume: VoxelVolume,
    props: &[EditorProp],
    structure: StructuralSystem,
    ground_system: Option<StructuralSystem>,
    max_span_m: f32,
    foundation_depth_m: Option<f32>,
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
    let bounds_m = volume.config().bounds_m();
    let stories = stats
        .rooms
        .iter()
        .map(|room| u16::try_from(room.floor.saturating_add(1)).unwrap_or(u16::MAX))
        .max()
        .unwrap_or_else(|| (bounds_m[1] / 3.2).ceil().max(1.0) as u16);
    let mut spec = StructureSpec::new(structure, stories, bounds_m[1], max_span_m);
    spec.ground_system = ground_system;
    if let Some(depth) = foundation_depth_m {
        spec.foundation_depth_m = depth;
    }
    metadata.structure = Some(spec);
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
    structure: StructuralSystem,
    ground_system: Option<StructuralSystem>,
    structure_span_m: f32,
    structure_foundation_m: Option<f32>,
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
            structure: StructuralSystem::TimberFrame,
            ground_system: None,
            structure_span_m: 0.0,
            structure_foundation_m: None,
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
    policies: Mutex<Option<DistrictPolicyStore>>,
    population: Mutex<Option<CityRuntime>>,
    named: Mutex<Option<NamedCitizens>>,
    materialization: Mutex<Option<MaterializationSelection>>,
    economy: Mutex<Option<Economy>>,
    industry: Mutex<Option<IndustryStore>>,
    utilities: Mutex<Option<UtilityStore>>,
    services: Mutex<Option<ServiceStore>>,
    research: Mutex<Option<ResearchStore>>,
    route_preview: Mutex<Option<RoutePreviewState>>,
    graph_cache: Mutex<Option<RoadGraphCache>>,
    movement_cache: Mutex<MovementCacheSlot>,
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
            policies: Mutex::new(None),
            population: Mutex::new(None),
            named: Mutex::new(None),
            materialization: Mutex::new(None),
            economy: Mutex::new(None),
            industry: Mutex::new(None),
            utilities: Mutex::new(None),
            services: Mutex::new(None),
            research: Mutex::new(None),
            route_preview: Mutex::new(None),
            graph_cache: Mutex::new(None),
            movement_cache: Mutex::new(MovementCacheSlot::default()),
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
        let seed = match city_layout_seed(seed) {
            Ok(seed) => seed,
            Err(message) => {
                let mut result = VarDictionary::new();
                result.set("error", &GString::from(message).to_variant());
                return result;
            }
        };
        let Some(tier) = city_tier_from_code(tier) else {
            let mut result = VarDictionary::new();
            let message = format!("invalid_city_tier: {tier}");
            result.set("error", &GString::from(&message).to_variant());
            return result;
        };
        match CityLayout::generate(tier, seed) {
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
    fn world_seed_error(&self, seed: u64) -> Option<VarDictionary> {
        let world = self.world.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(store) = world.as_ref() {
            if store.seed != seed {
                return Some(placement_simple_error(
                    "seed_mismatch",
                    "the placement store belongs to a different seed",
                ));
            }
        }
        None
    }

    fn research_seed_error(&self, seed: u64) -> Option<VarDictionary> {
        let research = self
            .research
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if research.as_ref().is_some_and(|store| store.seed != seed) {
            return Some(placement_simple_error(
                "seed_mismatch",
                "the research pool belongs to a different seed",
            ));
        }
        None
    }

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
        if let Some(error) = self.world_seed_error(seed) {
            return error;
        }
        let current_age =
            AgeState::from_population(variant_u64(&self.city_summary(), "population")).age;
        if let Some(structure) = request.structure.as_ref() {
            if let Some((code, error)) = structure_age_error(structure, current_age) {
                return placement_simple_error(code, error);
            }
        }
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
                result.set(
                    "structure",
                    &request
                        .structure
                        .as_ref()
                        .map_or_else(VarDictionary::new, structure_payload)
                        .to_variant(),
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
        let structure = structure_spec_for_blueprint(&blueprint);
        result.set(
            "structure",
            &structure
                .as_ref()
                .map_or_else(VarDictionary::new, structure_payload)
                .to_variant(),
        );
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
        if let Some(error) = self.world_seed_error(seed) {
            return error;
        }
        if let Some(error) = self.research_seed_error(seed) {
            return error;
        }
        let current_age =
            AgeState::from_population(variant_u64(&self.city_summary(), "population")).age;
        if let Some(structure) = request.structure.as_ref() {
            if let Some((code, error)) = structure_age_error(structure, current_age) {
                return placement_simple_error(code, error);
            }
        }
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
        {
            let economy = self
                .economy
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if economy.as_ref().is_some_and(|economy| economy.seed != seed) {
                return placement_simple_error(
                    "seed_mismatch",
                    "the treasury belongs to a different seed",
                );
            }
            let first_installment = first_construction_payment(request.material_cost);
            if let Some(economy) = economy.as_ref() {
                if economy.bankrupt {
                    return placement_simple_error(
                        "bankrupt",
                        "the city is bankrupt and cannot start new construction",
                    );
                }
                if !economy.can_afford(first_installment) {
                    return placement_simple_error(
                        "insufficient_funds",
                        format!(
                            "the first construction installment needs {:.0} but the treasury has {:.0}",
                            first_installment, economy.balance
                        ),
                    );
                }
            } else if first_installment > STARTING_BALANCE {
                return placement_simple_error(
                    "insufficient_funds",
                    format!(
                        "the first construction installment needs {:.0} but a new treasury has {:.0}",
                        first_installment, STARTING_BALANCE
                    ),
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
                    .add_priced_site(placement.id, request.solid_voxels, request.material_cost)
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
                let first_installment = first_construction_payment(request.material_cost);
                let marked_paid = {
                    let mut economy = self
                        .economy
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    let economy = economy.get_or_insert_with(|| Economy::new(seed));
                    let mut construction = self
                        .construction
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    settle_first_installment(
                        economy,
                        &mut construction,
                        placement.id,
                        first_installment,
                    )
                };
                if !marked_paid {
                    if let Some(store) = guard.as_mut() {
                        store.remove(placement.id);
                    }
                    self.construction
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .remove_site(placement.id);
                    if created {
                        *guard = None;
                    }
                    return placement_simple_error(
                        "insufficient_funds",
                        "the treasury changed while placing; construction was cancelled",
                    );
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

    /// Derived traffic graph statistics for the current road network.
    #[func]
    fn traffic_state(&self, seed: i64) -> VarDictionary {
        let Ok(seed) = u64::try_from(seed) else {
            return placement_simple_error("invalid_seed", "traffic seed must be non-negative");
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
        let Some(store) = roads.as_ref() else {
            return traffic_payload(&RoadGraph::default(), false);
        };
        if store.seed != seed {
            return placement_simple_error(
                "seed_mismatch",
                "the road store belongs to a different seed",
            );
        }
        let mut cache = self
            .graph_cache
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let graph = match cached_road_graph(&mut cache, seed, Some(store)) {
            Ok(graph) => graph,
            Err((code, message)) => return placement_simple_error(code, message),
        };
        traffic_payload(graph.as_ref(), true)
    }

    /// Read-only route preview between two world points.
    #[func]
    fn route_preview(
        &self,
        seed: i64,
        from_x: f64,
        from_z: f64,
        to_x: f64,
        to_z: f64,
    ) -> VarDictionary {
        let Ok(seed) = u64::try_from(seed) else {
            return placement_simple_error("invalid_seed", "traffic seed must be non-negative");
        };
        let from = [from_x as f32, from_z as f32];
        let to = [to_x as f32, to_z as f32];
        let limit = MAX_WORLD_COORD_M as f64;
        for value in [from_x, from_z, to_x, to_z] {
            if !value.is_finite() || value.abs() > limit {
                return placement_simple_error(
                    "coordinate_out_of_range",
                    "route coordinates must be finite and within the world bounds",
                );
            }
        }
        let world = self.world.lock().unwrap_or_else(|error| error.into_inner());
        if world.as_ref().is_some_and(|store| store.seed != seed) {
            return placement_simple_error(
                "seed_mismatch",
                "the world belongs to a different seed",
            );
        }
        let roads = self.roads.lock().unwrap_or_else(|error| error.into_inner());
        let Some(store) = roads.as_ref() else {
            return placement_simple_error("route_unavailable", "no road store is open");
        };
        if store.seed != seed {
            return placement_simple_error(
                "seed_mismatch",
                "the road store belongs to a different seed",
            );
        }
        let graph = {
            let mut cache = self
                .graph_cache
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            match cached_road_graph(&mut cache, seed, Some(store)) {
                Ok(graph) => graph,
                Err((code, message)) => return placement_simple_error(code, message),
            }
        };
        let graph_checksum = graph.checksum();
        let mut guard = self
            .route_preview
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if guard
            .as_ref()
            .is_none_or(|state| state.seed != seed || state.graph_checksum != graph_checksum)
        {
            *guard = Some(RoutePreviewState {
                seed,
                graph_checksum,
                cache: RouteCache::new(core_city::DEFAULT_ROUTE_CACHE_CAPACITY),
            });
        }
        let state = guard.as_mut().expect("route preview state");
        match route_preview_data(state, graph.as_ref(), from, to) {
            Ok(data) => {
                let mut points = PackedVector2Array::new();
                for point in &data.points {
                    points.push(Vector2::new(point[0], point[1]));
                }
                let mut result = VarDictionary::new();
                result.set("ok", true);
                result.set("seed", seed as i64);
                result.set("from", &PackedFloat32Array::from(from));
                result.set("to", &PackedFloat32Array::from(to));
                result.set("points", &points);
                result.set("length_m", f64::from(data.length_m));
                result.set("time_s", f64::from(data.time_s));
                result.set("cached", data.cached);
                result.set("cache_hits", data.hits as i64);
                result.set("cache_misses", data.misses as i64);
                result.set(
                    "checksum",
                    &GString::from(&format!("{:016x}", data.checksum)),
                );
                result
            }
            Err(code) => placement_simple_error(code, "no route connects the two points"),
        }
    }

    /// Route cache counters and state.
    #[func]
    fn route_cache_state(&self) -> VarDictionary {
        let guard = self
            .route_preview
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut result = VarDictionary::new();
        result.set("ok", true);
        match guard.as_ref() {
            Some(state) => {
                result.set("open", true);
                result.set("seed", state.seed as i64);
                result.set(
                    "graph_checksum",
                    &GString::from(&format!("{:016x}", state.graph_checksum)),
                );
                result.set("entries", state.cache.len() as i64);
                result.set("capacity", state.cache.capacity() as i64);
                result.set("hits", state.cache.hits() as i64);
                result.set("misses", state.cache.misses() as i64);
                result.set(
                    "checksum",
                    &GString::from(&format!("{:016x}", state.cache.checksum())),
                );
            }
            None => {
                result.set("open", false);
                result.set("seed", 0);
                result.set("graph_checksum", &GString::from("0000000000000000"));
                result.set("entries", 0);
                result.set("capacity", 0);
                result.set("hits", 0);
                result.set("misses", 0);
                result.set("checksum", &GString::from("0000000000000000"));
            }
        }
        result
    }

    /// Drop the route preview cache and its counters.
    #[func]
    fn route_cache_clear(&self) -> bool {
        *self
            .route_preview
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        true
    }

    /// Cached derived road graph identity and statistics.
    #[func]
    fn road_graph_cache_state(&self) -> VarDictionary {
        let guard = self
            .graph_cache
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut result = VarDictionary::new();
        result.set("ok", true);
        match guard.as_ref() {
            Some(cached) => {
                let stats = cached.graph.stats();
                result.set("open", true);
                result.set("seed", cached.seed as i64);
                result.set(
                    "roads_checksum",
                    &GString::from(&format!("{:016x}", cached.roads_checksum)),
                );
                result.set(
                    "graph_checksum",
                    &GString::from(&format!("{:016x}", cached.graph.checksum())),
                );
                result.set("nodes", stats.nodes as i64);
                result.set("edges", stats.edges as i64);
                result.set("roads", stats.roads as i64);
            }
            None => {
                result.set("open", false);
                result.set("seed", 0);
                result.set("roads_checksum", &GString::from("0000000000000000"));
                result.set("graph_checksum", &GString::from("0000000000000000"));
                result.set("nodes", 0);
                result.set("edges", 0);
                result.set("roads", 0);
            }
        }
        result
    }

    /// Drop the cached derived road graph.
    #[func]
    fn road_graph_cache_clear(&self) -> bool {
        *self
            .graph_cache
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        true
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
        if !lot_store_seed_matches(lots.as_ref(), seed) {
            return placement_simple_error(
                "seed_mismatch",
                "the lot store belongs to a different seed",
            );
        }
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
        self.utilities
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_mut()
            .map(UtilityStore::clear);
        self.services
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_mut()
            .map(ServiceStore::clear);
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
            if let Some(utilities) = self
                .utilities
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_mut()
            {
                utilities.remove_placement(id);
            }
            if let Some(services) = self
                .services
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_mut()
            {
                services.remove_placement(id);
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
        let payments = {
            let mut economy = self
                .economy
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            settle_construction_payments(&mut store, economy.as_mut())
        };
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
        result.set("paid_total", payments.paid_total);
        result.set("paid_sites", i64::from(payments.paid_sites));
        result.set("payment_stalled", i64::from(payments.payment_stalled));
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
            sites.push(&construction_site_to_dictionary(
                site,
                store.site_trade(site.placement_id),
            ));
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
        result.set("payment_due", store.total_payment_due());
        result.set(
            "payment_stalled",
            store
                .sites
                .values()
                .filter(|site| !site.is_complete() && site.payment_stalled)
                .count() as i64,
        );
        result.set("crew_capacity", i64::from(store.crew_capacity));
        result.set("effective_crew", i64::from(store.effective_crew_capacity()));
        let mut trades: Array<VarDictionary> = Array::new();
        if let Some(pool) = &store.trades {
            for trade in Trade::ALL {
                let count = pool.get(trade);
                if count > 0 {
                    let mut item = VarDictionary::new();
                    item.set("trade", &GString::from(trade.as_str()));
                    item.set("name", &GString::from(trade.name()));
                    item.set("count", i64::from(count));
                    trades.push(&item);
                }
            }
        }
        result.set("trades", &trades.to_variant());
        result.set("materials_stock", store.materials_stock as i64);
        result.set("time_scale", &GString::from(store.time_scale().name()));
        result.set(
            "seconds_per_game_day",
            store.time_scale().seconds_per_game_day().unwrap_or(0.0),
        );
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
        result.set(
            "average_commute_time_s",
            f64::from(summary.average_commute_time_s),
        );
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
        let traffic = self
            .roads
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .filter(|store| store.seed == seed)
            .and_then(|store| RoadGraph::build(&store.roads).ok());
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
        let (water_supply, power_supply) = {
            let utilities = self
                .utilities
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            utilities
                .as_ref()
                .filter(|utilities| utilities.seed == seed)
                .map_or((0.0, 0.0), |utilities| {
                    (
                        f64::from(utilities.water_supply),
                        f64::from(utilities.power_supply),
                    )
                })
        };
        let food_supply = {
            let industry = self
                .industry
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            industry
                .as_ref()
                .filter(|industry| industry.seed == seed)
                .map_or(0.0, |industry| industry.food_supply)
        };
        let report = runtime.tick_with_supplies_and_roads(
            seconds,
            &homes,
            &workplaces,
            food_supply,
            water_supply as f32,
            power_supply as f32,
            traffic.as_ref(),
        );
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("moved_in", i64::from(report.moved_in));
        result.set("population", report.population as i64);
        result.set("vacancies", report.vacancies as i64);
        result.set("workers", report.workers as i64);
        result.set("happiness", f64::from(runtime.happiness()));
        result.set("average_commute_m", f64::from(runtime.average_commute_m()));
        result.set(
            "average_commute_time_s",
            f64::from(runtime.average_commute_time_s()),
        );
        result.set("daily_wages", runtime.daily_wages());
        result.set("water_supply", water_supply);
        result.set("power_supply", power_supply);
        result.set(
            "checksum",
            &GString::from(&format!("{:016x}", runtime.checksum())),
        );
        result
    }

    /// Aggregate metrics for one circular district query.
    ///
    /// This is a read-only derived snapshot: no save field, no checksum change,
    /// and no district UI yet. Lock order is world -> construction -> population.
    #[func]
    fn district_summary(
        &self,
        seed: i64,
        center_x: f64,
        center_z: f64,
        radius_m: f64,
    ) -> VarDictionary {
        let Ok(seed) = u64::try_from(seed) else {
            return placement_simple_error("invalid_seed", "world seed must be non-negative");
        };
        let center = [center_x as f32, center_z as f32];
        if !center.iter().all(|value| value.is_finite()) {
            return placement_simple_error("invalid_center", "district center must be finite");
        }
        let max_radius_m = f64::from(MAX_WORLD_COORD_M);
        if !radius_m.is_finite() || !(0.0..=max_radius_m).contains(&radius_m) {
            return placement_simple_error(
                "invalid_radius",
                "district radius must be finite and within 0..=1,000,000 m",
            );
        }
        let placements = self.world.lock().unwrap_or_else(|error| error.into_inner());
        if placements.as_ref().is_some_and(|store| store.seed != seed) {
            return placement_simple_error(
                "seed_mismatch",
                "the placement store belongs to a different seed",
            );
        }
        let construction = self
            .construction
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let population = self
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
        let summary = match placements.as_ref() {
            Some(store) => {
                let empty_runtime = CityRuntime::new(seed);
                let runtime = population.as_ref().unwrap_or(&empty_runtime);
                summarize_district(store, &construction, runtime, center, radius_m as f32)
            }
            None => DistrictSummary::default(),
        };
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("seed", seed as i64);
        result.set("center", &PackedFloat32Array::from(center));
        result.set("radius_m", radius_m);
        result.set("population", summary.population as i64);
        result.set("housing_capacity", summary.housing_capacity as i64);
        result.set("vacancies", summary.vacancies as i64);
        result.set("households", summary.households as i64);
        result.set("jobs", summary.jobs as i64);
        result.set("employed", summary.employed as i64);
        result.set("unemployed", summary.unemployed as i64);
        result.set("happiness", f64::from(summary.happiness));
        result.set("food", f64::from(summary.food));
        result.set("water", f64::from(summary.water));
        result.set("rest", f64::from(summary.rest));
        result.set("average_commute_m", f64::from(summary.average_commute_m));
        result.set("completed_buildings", summary.completed_buildings as i64);
        result.set("construction_sites", summary.construction_sites as i64);
        result.set(
            "checksum",
            &GString::from(&format!("{:016x}", summary.checksum())),
        );
        result
    }

    /// Dense heatmap data for one circular district query.
    ///
    /// Read-only derived state: no save field and no checksum change. The
    /// grid is row-major with at most 4096 cells. Lock order is
    /// world -> construction -> population, matching `district_summary`.
    #[func]
    fn district_heatmap(
        &self,
        seed: i64,
        center_x: f64,
        center_z: f64,
        radius_m: f64,
        cell_m: f64,
    ) -> VarDictionary {
        let Ok(seed) = u64::try_from(seed) else {
            return placement_simple_error("invalid_seed", "world seed must be non-negative");
        };
        let center = [center_x as f32, center_z as f32];
        if !center.iter().all(|value| value.is_finite()) {
            return placement_simple_error("invalid_center", "district center must be finite");
        }
        let max_radius_m = f64::from(MAX_WORLD_COORD_M);
        if !radius_m.is_finite() || !(0.0..=max_radius_m).contains(&radius_m) {
            return placement_simple_error(
                "invalid_radius",
                "district radius must be finite and within 0..=1,000,000 m",
            );
        }
        let min_cell_m = f64::from(MIN_HEATMAP_CELL_M);
        let max_cell_m = f64::from(MAX_HEATMAP_CELL_M);
        if !cell_m.is_finite() || !(min_cell_m..=max_cell_m).contains(&cell_m) {
            return placement_simple_error(
                "invalid_cell_size",
                format!(
                    "district heatmap cell size must be finite and within {MIN_HEATMAP_CELL_M}..={MAX_HEATMAP_CELL_M} m"
                ),
            );
        }
        if heatmap_grid(radius_m as f32, cell_m as f32).is_none() {
            return placement_simple_error(
                "invalid_grid",
                format!(
                    "district heatmap grid exceeds {MAX_HEATMAP_CELLS} cells; increase cell_m or shrink the radius"
                ),
            );
        }

        let placements = self.world.lock().unwrap_or_else(|error| error.into_inner());
        if placements.as_ref().is_some_and(|store| store.seed != seed) {
            return placement_simple_error(
                "seed_mismatch",
                "the placement store belongs to a different seed",
            );
        }
        let construction = self
            .construction
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let population = self
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

        let empty_runtime = CityRuntime::new(seed);
        let runtime = population.as_ref().unwrap_or(&empty_runtime);
        let heatmap = match placements.as_ref() {
            Some(store) => summarize_heatmap(
                store,
                &construction,
                runtime,
                center,
                radius_m as f32,
                cell_m as f32,
            ),
            None => {
                let empty_store = PlacementStore::new(seed);
                summarize_heatmap(
                    &empty_store,
                    &construction,
                    runtime,
                    center,
                    radius_m as f32,
                    cell_m as f32,
                )
            }
        };
        let payload = district_heatmap_payload(seed, &heatmap);
        district_heatmap_payload_to_dictionary(&payload)
    }

    /// Create one district policy in the in-memory registry.
    ///
    /// The registry is created lazily on the first policy for `seed`. It is
    /// data only: no simulation, household, treasury, or economy code reads it.
    /// The registry is persisted as an optional `policies` object by
    /// [`Self::world_save`] and restored by [`Self::world_load`], but saving
    /// does not apply any policy effect. The allowlist is a
    /// `PackedStringArray` of namespaced blueprint ids. On failure the lazy
    /// registry is removed again, so a rejected first create leaves no store.
    #[func]
    #[allow(clippy::too_many_arguments)]
    fn district_policy_create(
        &self,
        seed: i64,
        name: GString,
        center_x: f64,
        center_z: f64,
        radius_m: f64,
        tax_multiplier: f64,
        service_funding: f64,
        construction_allowed: bool,
        allowlist: PackedStringArray,
    ) -> VarDictionary {
        let Ok(seed) = u64::try_from(seed) else {
            return placement_simple_error("invalid_seed", "world seed must be non-negative");
        };
        let request = district_policy_request(
            name,
            center_x,
            center_z,
            radius_m,
            tax_multiplier,
            service_funding,
            construction_allowed,
            &allowlist,
        );
        let mut guard = self
            .policies
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match district_policy_create_in(&mut guard, seed, &request) {
            Ok(policy) => district_policy_result(seed, &policy),
            Err(error) => district_policy_error_result(&error),
        }
    }

    /// Replace every mutable field of one district policy; the id is fixed.
    ///
    /// Data only: see [`Self::district_policy_create`]. The registry must
    /// already be open for this seed.
    #[func]
    #[allow(clippy::too_many_arguments)]
    fn district_policy_update(
        &self,
        seed: i64,
        id: i64,
        name: GString,
        center_x: f64,
        center_z: f64,
        radius_m: f64,
        tax_multiplier: f64,
        service_funding: f64,
        construction_allowed: bool,
        allowlist: PackedStringArray,
    ) -> VarDictionary {
        let Ok(seed) = u64::try_from(seed) else {
            return placement_simple_error("invalid_seed", "world seed must be non-negative");
        };
        let Ok(id) = u64::try_from(id) else {
            return placement_simple_error("not_found", "district policy id must be non-negative");
        };
        let request = district_policy_request(
            name,
            center_x,
            center_z,
            radius_m,
            tax_multiplier,
            service_funding,
            construction_allowed,
            &allowlist,
        );
        let mut guard = self
            .policies
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match district_policy_update_in(guard.as_mut(), seed, id, &request) {
            Ok(policy) => district_policy_result(seed, &policy),
            Err(error) => district_policy_error_result(&error),
        }
    }

    /// Remove one district policy by id.
    ///
    /// Data only: see [`Self::district_policy_create`]. A missing registry or
    /// id returns a `not_found` error; a different registry seed returns
    /// `seed_mismatch`.
    #[func]
    fn district_policy_remove(&self, seed: i64, id: i64) -> VarDictionary {
        let Ok(seed) = u64::try_from(seed) else {
            return placement_simple_error("invalid_seed", "world seed must be non-negative");
        };
        let Ok(id) = u64::try_from(id) else {
            return placement_simple_error("not_found", "district policy id must be non-negative");
        };
        let mut guard = self
            .policies
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (count, checksum) = match district_policy_remove_in(guard.as_mut(), seed, id) {
            Ok(result) => result,
            Err(error) => return district_policy_error_result(&error),
        };
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("seed", seed as i64);
        result.set("id", id as i64);
        result.set("count", count as i64);
        result.set("checksum", &GString::from(&format!("{checksum:016x}")));
        result
    }

    /// Return the district policy registry for one seed.
    ///
    /// A registry that has never been created reports `count: 0`, an empty
    /// `policies` array, and a zero checksum. A registry from another seed is
    /// a `seed_mismatch`. Data only: no simulation or UI consumer reads these
    /// values yet, and saving them applies no tax, funding, construction, or
    /// allowlist effect.
    #[func]
    fn district_policies_state(&self, seed: i64) -> VarDictionary {
        let Ok(seed) = u64::try_from(seed) else {
            return placement_simple_error("invalid_seed", "world seed must be non-negative");
        };
        let guard = self
            .policies
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match district_policies_state_payload(guard.as_ref(), seed) {
            Ok(Some(payload)) => district_policies_payload_to_dictionary(&payload),
            Ok(None) => {
                let mut result = VarDictionary::new();
                result.set("ok", true);
                result.set("seed", seed as i64);
                result.set("count", 0);
                result.set("policies", &Array::<VarDictionary>::new().to_variant());
                result.set("checksum", &GString::from("0000000000000000"));
                result
            }
            Err(error) => district_policy_error_result(&error),
        }
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
                result.set(
                    "average_commute_time_s",
                    f64::from(runtime.average_commute_time_s()),
                );
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
                    item.set("commute_time_s", f64::from(household.commute_time_s));
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
                result.set("average_commute_m", 0.0);
                result.set("average_commute_time_s", 0.0);
                result.set("daily_wages", 0.0);
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

    /// Derived named-citizen counts, age metrics, and checksum.
    ///
    /// Age fields are read-only views over the synced pool: `average_age_days`,
    /// `average_age_years`, `oldest_age_days`, and `oldest_age_years`, all
    /// truncated to whole units. An empty pool reports zero for every age
    /// field.
    #[func]
    fn named_citizens_state(&self, seed: i64) -> VarDictionary {
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
        let population = self
            .population
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut named = self.named.lock().unwrap_or_else(|error| error.into_inner());
        if named.as_ref().is_some_and(|named| named.seed != seed) {
            *named = None;
        }
        let pool = named.get_or_insert_with(|| NamedCitizens::new(seed));
        match population.as_ref().filter(|runtime| runtime.seed == seed) {
            Some(runtime) => pool.sync(&runtime.households),
            None => pool.clear(),
        }
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("open", true);
        result.set("seed", seed as i64);
        result.set("count", pool.len() as i64);
        result.set("max", pool.max as i64);
        let average_age_days = pool.average_age_days();
        let oldest_age_days = pool.oldest_age_days().unwrap_or(0);
        result.set("average_age_days", i64::from(average_age_days));
        result.set(
            "average_age_years",
            i64::from(age_years_from_days(average_age_days)),
        );
        result.set("oldest_age_days", i64::from(oldest_age_days));
        result.set(
            "oldest_age_years",
            i64::from(age_years_from_days(oldest_age_days)),
        );
        result.set(
            "checksum",
            &GString::from(&format!("{:016x}", pool.checksum())),
        );
        result
    }

    /// Named residents of one home, sorted by slot.
    ///
    /// Each citizen carries `id`, `name`, `slot`, persisted `age_days`, derived
    /// `age_years` (`age_days / 365`), and `employed` (the first
    /// `household.workers` slots). The household lookup is read-only; the age is
    /// a view over the synced pool and is not written back. A missing household
    /// reports `employed = false`.
    #[func]
    fn named_citizens_for(&self, seed: i64, placement_id: i64) -> VarDictionary {
        let Ok(seed) = u64::try_from(seed) else {
            return placement_simple_error("invalid_seed", "world seed must be non-negative");
        };
        let Ok(placement_id) = u64::try_from(placement_id) else {
            return placement_simple_error(
                "invalid_placement",
                "placement id must be non-negative",
            );
        };
        let state = self.named_citizens_state(seed as i64);
        if !variant_bool(&state, "ok") {
            return state;
        }
        let population = self
            .population
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let household = population
            .as_ref()
            .filter(|runtime| runtime.seed == seed)
            .and_then(|runtime| runtime.households.get(&placement_id));
        let named = self.named.lock().unwrap_or_else(|error| error.into_inner());
        let mut citizens: Array<VarDictionary> = Array::new();
        if let Some(pool) = named.as_ref() {
            for citizen in pool.citizens_of(placement_id) {
                let mut item = VarDictionary::new();
                item.set("id", citizen.id as i64);
                item.set("name", &GString::from(citizen.name.as_str()));
                item.set("slot", i64::from(citizen.slot));
                item.set("age_days", i64::from(citizen.age_days));
                item.set(
                    "age_years",
                    i64::from(age_years_from_days(citizen.age_days)),
                );
                item.set(
                    "employed",
                    household.is_some_and(|household| slot_is_employed(household, citizen.slot)),
                );
                citizens.push(&item);
            }
        }
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("seed", seed as i64);
        result.set("placement_id", placement_id as i64);
        result.set("count", citizens.len() as i64);
        result.set("citizens", &citizens.to_variant());
        result
    }

    /// Advance the named pool by whole simulated days and report aging/death.
    ///
    /// `days` must be finite and within `0..=MAX_AGE_DAYS` (36,500). The core
    /// floors it to whole days, so a sub-day value is a no-op. Only the named
    /// pool is touched: households, population, and the save state are
    /// unchanged, and applying deaths to household residents is a caller
    /// concern.
    ///
    /// Returns `{ok, seed, aged, died, count, average_age_days,
    /// oldest_age_days, checksum, deaths}`. `aged` counts every citizen that
    /// received the advance, including citizens removed in the same tick;
    /// `died` counts the removals. `deaths` is an array of
    /// `{id, placement_id, slot, name, age_days}` records captured before the
    /// household sync, ordered ascending by `(placement_id, slot)`. It is empty
    /// when nobody dies. The array is the deterministic sequence for the tick;
    /// there is no persisted day or event journal.
    #[func]
    fn named_lifecycle_tick(&self, seed: i64, days: f64) -> VarDictionary {
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
        let max_days = f64::from(MAX_AGE_DAYS);
        if !days.is_finite() || !(0.0..=max_days).contains(&days) {
            return placement_simple_error(
                "invalid_days",
                "days must be finite and within 0..=36500",
            );
        }
        let mut population = self
            .population
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(runtime) = population.as_mut() else {
            return placement_simple_error("no_population", "no household runtime is open");
        };
        if runtime.seed != seed {
            return placement_simple_error(
                "seed_mismatch",
                "the household runtime belongs to a different seed",
            );
        }
        let mut named = self.named.lock().unwrap_or_else(|error| error.into_inner());
        if named.as_ref().is_some_and(|named| named.seed != seed) {
            *named = None;
        }
        let pool = named.get_or_insert_with(|| NamedCitizens::new(seed));
        let outcome = tick_named_lifecycle(pool, runtime, days);
        let mut deaths = VarArray::new();
        for event in &outcome.death_events {
            deaths.push(&citizen_death_to_dictionary(event));
        }
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("seed", seed as i64);
        result.set("aged", outcome.tick.aged as i64);
        result.set("died", outcome.tick.died as i64);
        result.set("applied_deaths", outcome.applied_deaths as i64);
        result.set("population", outcome.population as i64);
        result.set("count", outcome.count as i64);
        result.set("average_age_days", i64::from(outcome.average_age_days));
        result.set("oldest_age_days", i64::from(outcome.oldest_age_days));
        result.set("deaths", &deaths.to_variant());
        result.set(
            "checksum",
            &GString::from(&format!("{:016x}", outcome.checksum)),
        );
        result
    }

    /// Read-only active/detailed materialization selection around a camera point.
    ///
    /// Returns `{ok, seed, center:[x,z], counts:{active,detailed}, active,
    /// detailed}`. Each citizen has `id`, `name`, `placement_id`, `slot`,
    /// `distance_m`, and `home:[x,z]`. The detailed list is a prefix of the
    /// active list.
    ///
    /// The named pool is synced on a local copy: this call does not mutate the
    /// named pool, the household runtime, or the save state.
    #[func]
    fn materialization_state(
        &self,
        seed: i64,
        center_x: f64,
        center_z: f64,
        active_radius_m: f64,
        detailed_radius_m: f64,
    ) -> VarDictionary {
        let Ok(seed) = u64::try_from(seed) else {
            return placement_simple_error("invalid_seed", "world seed must be non-negative");
        };
        let center = [center_x as f32, center_z as f32];
        if !center.iter().all(|value| value.is_finite()) {
            return placement_simple_error(
                "invalid_center",
                "materialization center must be finite",
            );
        }
        let max_radius_m = f64::from(MAX_WORLD_COORD_M);
        if !active_radius_m.is_finite() || !(0.0..=max_radius_m).contains(&active_radius_m) {
            return placement_simple_error(
                "invalid_radius",
                "active materialization radius must be finite and within 0..=1,000,000 m",
            );
        }
        if !detailed_radius_m.is_finite() || !(0.0..=max_radius_m).contains(&detailed_radius_m) {
            return placement_simple_error(
                "invalid_radius",
                "detailed materialization radius must be finite and within 0..=1,000,000 m",
            );
        }

        let placement_centers = {
            let world = self.world.lock().unwrap_or_else(|error| error.into_inner());
            if world.as_ref().is_some_and(|store| store.seed != seed) {
                return placement_simple_error(
                    "seed_mismatch",
                    "the placement store belongs to a different seed",
                );
            }
            let construction = self
                .construction
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            world.as_ref().map_or_else(BTreeMap::new, |store| {
                completed_homes(store, &construction)
                    .into_iter()
                    .map(|home| (home.id, home.center))
                    .collect()
            })
        };

        let population = self
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
        let named = self.named.lock().unwrap_or_else(|error| error.into_inner());
        let empty = BTreeMap::new();
        let households = population
            .as_ref()
            .map_or(&empty, |runtime| &runtime.households);
        let payload = match materialization_payload(
            seed,
            named.as_ref(),
            households,
            &placement_centers,
            center,
            active_radius_m as f32,
            detailed_radius_m as f32,
        ) {
            Ok(payload) => payload,
            Err(error) => {
                return placement_simple_error("materialization_failed", error.to_string())
            }
        };
        materialization_payload_to_dictionary(&payload)
    }

    /// Advance the stateful N3 materialization selection.
    ///
    /// Unlike `materialization_state`, this keeps the previous selection,
    /// applies hysteresis, and returns a 64-change diff. The call is
    /// deterministic and does not mutate the named pool or save state.
    #[func]
    #[allow(clippy::too_many_arguments)]
    fn materialization_step(
        &self,
        seed: i64,
        center_x: f64,
        center_z: f64,
        inner_radius_m: f64,
        outer_radius_m: f64,
        detailed_radius_m: f64,
        pinned_id: i64,
    ) -> VarDictionary {
        let Ok(seed) = u64::try_from(seed) else {
            return placement_simple_error("invalid_seed", "world seed must be non-negative");
        };
        let center = [center_x as f32, center_z as f32];
        if !center.iter().all(|value| value.is_finite()) {
            return placement_simple_error(
                "invalid_center",
                "materialization center must be finite",
            );
        }
        let max_radius = f64::from(MAX_WORLD_COORD_M);
        for (label, value) in [
            ("inner", inner_radius_m),
            ("outer", outer_radius_m),
            ("detailed", detailed_radius_m),
        ] {
            if !value.is_finite() || !(0.0..=max_radius).contains(&value) {
                return placement_simple_error(
                    "invalid_radius",
                    format!("{label} radius must be finite and within 0..=1,000,000 m"),
                );
            }
        }
        if inner_radius_m > outer_radius_m {
            return placement_simple_error("invalid_radius", "inner radius exceeds outer radius");
        }
        let pinned = if pinned_id == 0 {
            None
        } else {
            match u64::try_from(pinned_id) {
                Ok(id) => Some(id),
                Err(_) => {
                    return placement_simple_error("invalid_pin", "pinned id must be non-negative")
                }
            }
        };

        let placement_centers = {
            let world = self.world.lock().unwrap_or_else(|error| error.into_inner());
            if world.as_ref().is_some_and(|store| store.seed != seed) {
                return placement_simple_error(
                    "seed_mismatch",
                    "the placement store belongs to a different seed",
                );
            }
            let construction = self
                .construction
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            world.as_ref().map_or_else(BTreeMap::new, |store| {
                completed_homes(store, &construction)
                    .into_iter()
                    .map(|home| (home.id, home.center))
                    .collect()
            })
        };
        let population = self
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
        let named = self.named.lock().unwrap_or_else(|error| error.into_inner());
        let mut step = self
            .materialization
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let empty = BTreeMap::new();
        let households = population
            .as_ref()
            .map_or(&empty, |runtime| &runtime.households);
        let Some(pool) = named.as_ref().filter(|pool| pool.seed == seed) else {
            return placement_simple_error("no_named_citizens", "named pool is not initialized");
        };
        let mut pool = pool.clone();
        pool.sync(households);
        let previous = step.as_ref();
        let next = match pool.select_near_with_previous(
            previous,
            center,
            &placement_centers,
            MaterializationBudget::default(),
            inner_radius_m as f32,
            outer_radius_m as f32,
            detailed_radius_m as f32,
            pinned,
        ) {
            Ok(next) => next,
            Err(error) => {
                return placement_simple_error("materialization_failed", error.to_string())
            }
        };
        let diff = diff_selection(previous, &next, 64);
        *step = Some(next.clone());
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("seed", seed as i64);
        result.set("center", &PackedFloat32Array::from(center));
        let mut counts = VarDictionary::new();
        counts.set("active", next.active.len() as i64);
        counts.set("detailed", next.detailed.len() as i64);
        result.set("counts", &counts);
        let mut active: Array<VarDictionary> = Array::new();
        for citizen in &next.active {
            active.push(&materialized_citizen_to_dictionary(citizen));
        }
        let mut detailed: Array<VarDictionary> = Array::new();
        for citizen in &next.detailed {
            detailed.push(&materialized_citizen_to_dictionary(citizen));
        }
        result.set("active", &active.to_variant());
        result.set("detailed", &detailed.to_variant());
        let mut diff_counts = VarDictionary::new();
        diff_counts.set("added", diff.added.len() as i64);
        diff_counts.set("removed", diff.removed.len() as i64);
        diff_counts.set("updated", diff.updated.len() as i64);
        diff_counts.set("retained", diff.retained as i64);
        diff_counts.set("changed", diff.changed as i64);
        diff_counts.set("applied", diff.applied as i64);
        diff_counts.set("truncated", diff.truncated);
        result.set("diff", &diff_counts);
        let mut changed: Array<VarDictionary> = Array::new();
        for citizen in &diff.added {
            let mut item = materialized_citizen_to_dictionary(citizen);
            item.set("action", &GString::from("added"));
            changed.push(&item);
        }
        for citizen in &diff.updated {
            let mut item = materialized_citizen_to_dictionary(citizen);
            item.set("action", &GString::from("updated"));
            changed.push(&item);
        }
        for id in &diff.removed {
            let mut item = VarDictionary::new();
            item.set("action", &GString::from("removed"));
            item.set("id", *id as i64);
            changed.push(&item);
        }
        result.set("changed", &changed.to_variant());
        result.set(
            "checksum",
            &GString::from(&format!("{:016x}", pool.checksum())),
        );
        result
    }

    /// Forget the stored materialization selection.
    #[func]
    fn materialization_clear(&self) -> bool {
        *self
            .materialization
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        true
    }

    /// Read-only per-citizen positions around a camera point at `time_s`.
    ///
    /// Selects the active residents exactly like `materialization_state`, then
    /// builds the `core_city::movement` layer: one cached home-work route per
    /// household, shared by its residents, sampled at `time_s`.
    ///
    /// Returns `{ok, seed, center:[x,z], time_s, routes, citizens,
    /// positions:[{id, placement_id, slot, position:[x,z], leg,
    /// route_progress}], checksum}`. `leg` is `"outbound"` or `"return"`.
    /// Households without a reachable workplace are omitted, not reported as
    /// errors, so `citizens` may be lower than the materialization count.
    ///
    /// `time_s` is finite presentation time in `0..=1,000,000` s (about 11.6
    /// days). The bound keeps the periodic reduction well-conditioned and
    /// rejects clock glitches; motion itself wraps within each household cycle.
    ///
    /// A missing road store returns `no_roads`. The call is read-only: it does
    /// not mutate the named pool, population, roads, or save state.
    ///
    /// The payload also contains `route_polylines`, one entry per routed
    /// household in placement-id order, with its home/workplace ids, points,
    /// outbound length, and round-trip cycle. It is display data only.
    #[func]
    fn citizen_movement(
        &self,
        seed: i64,
        center_x: f64,
        center_z: f64,
        active_radius_m: f64,
        detailed_radius_m: f64,
        time_s: f64,
    ) -> VarDictionary {
        let request = match movement_request(
            seed,
            center_x,
            center_z,
            active_radius_m,
            detailed_radius_m,
            time_s,
        ) {
            Ok(request) => request,
            Err((code, message)) => return placement_simple_error(code, message),
        };

        let (placement_centers, workplaces, world_seed, world_checksum, construction_checksum) = {
            let world = self.world.lock().unwrap_or_else(|error| error.into_inner());
            let construction = self
                .construction
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let construction_checksum = construction.checksum();
            match world.as_ref() {
                Some(store) => (
                    completed_homes(store, &construction)
                        .into_iter()
                        .map(|home| (home.id, home.center))
                        .collect(),
                    completed_workplaces(store, &construction),
                    Some(store.seed),
                    store.checksum(),
                    construction_checksum,
                ),
                None => (BTreeMap::new(), Vec::new(), None, 0, construction_checksum),
            }
        };
        let population = self
            .population
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some((code, message)) = movement_seed_error(
            request.seed,
            world_seed,
            population.as_ref().map(|runtime| runtime.seed),
        ) {
            return placement_simple_error(code, message);
        }
        let named = self.named.lock().unwrap_or_else(|error| error.into_inner());
        let roads = self.roads.lock().unwrap_or_else(|error| error.into_inner());
        let graph = {
            let mut cache = self
                .graph_cache
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            match cached_road_graph(&mut cache, request.seed, roads.as_ref()) {
                Ok(graph) => graph,
                Err((code, message)) => return placement_simple_error(code, message),
            }
        };
        let key = MovementCacheKey {
            seed: request.seed,
            world_checksum,
            construction_checksum,
            population_checksum: population.as_ref().map_or(0, CityRuntime::checksum),
            workplace_checksum: population
                .as_ref()
                .map_or(0, CityRuntime::workplace_checksum),
            named_checksum: named.as_ref().map_or(0, NamedCitizens::checksum),
            roads_checksum: roads.as_ref().map_or(0, RoadStore::checksum),
            center: [request.center[0].to_bits(), request.center[1].to_bits()],
            active_radius: request.active_radius_m.to_bits(),
            detailed_radius: request.detailed_radius_m.to_bits(),
        };
        let mut movement_cache = self
            .movement_cache
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(cache) = movement_cache.lookup(&key) {
            let payload = movement_payload_from_motion(
                request.seed,
                request.center,
                request.time_s,
                &cache.motion,
            );
            return movement_payload_to_dictionary(&payload);
        }
        let empty = BTreeMap::new();
        let households = population
            .as_ref()
            .map_or(&empty, |runtime| &runtime.households);
        let motion = match movement_motion(
            request.seed,
            named.as_ref(),
            households,
            &placement_centers,
            &workplaces,
            &graph,
            request.center,
            request.active_radius_m,
            request.detailed_radius_m,
        ) {
            Ok(motion) => motion,
            Err(error) => {
                return placement_simple_error("materialization_failed", error.to_string())
            }
        };
        let payload =
            movement_payload_from_motion(request.seed, request.center, request.time_s, &motion);
        movement_cache.store(key, motion);
        movement_payload_to_dictionary(&payload)
    }

    /// Movement cache counters and key.
    ///
    /// `hits`, `misses`, `builds`, and `clears` are cumulative for the lifetime
    /// of this bridge instance. `movement_cache_clear` drops the cached entry
    /// and increments `clears` but keeps the other three counters, matching
    /// `core_city::RouteCache::clear`.
    #[func]
    fn movement_cache_state(&self) -> VarDictionary {
        let guard = self
            .movement_cache
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let stats = guard.stats();
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("open", stats.open);
        result.set("seed", stats.seed as i64);
        result.set("routes", stats.routes as i64);
        result.set("citizens", stats.citizens as i64);
        result.set("hits", stats.hits as i64);
        result.set("misses", stats.misses as i64);
        result.set("builds", stats.builds as i64);
        result.set("clears", stats.clears as i64);
        result.set(
            "checksum",
            &GString::from(&format!("{:016x}", stats.checksum)),
        );
        result
    }

    /// Drop the cached movement tracks and count the clear.
    ///
    /// The `hits`, `misses`, and `builds` counters stay cumulative.
    #[func]
    fn movement_cache_clear(&self) -> bool {
        self.movement_cache
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
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

    /// Advance utility and civic lead times by `days` for one seed.
    ///
    /// Only existing stores for the matching seed are touched, so a closed or
    /// foreign store is left alone. Errors come from invalid day steps.
    fn advance_infrastructure_lead(&self, seed: u64, days: f32) -> Result<(), String> {
        {
            let mut guard = self
                .utilities
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(store) = guard.as_mut().filter(|store| store.seed == seed) {
                store.tick(days).map_err(|error| error.to_string())?;
            }
        }
        {
            let mut guard = self
                .services
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(store) = guard.as_mut().filter(|store| store.seed == seed) {
                store.tick(days).map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    }

    fn utility_infrastructure_for_seed(&self, seed: u64) -> (u64, f64) {
        let guard = self
            .utilities
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        guard
            .as_ref()
            .filter(|store| store.seed == seed)
            .map_or((0, 0.0), |store| {
                (store.site_count() as u64, store.total_upkeep_per_day())
            })
    }

    fn service_infrastructure_for_seed(&self, seed: u64) -> (u64, f64) {
        let guard = self
            .services
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        guard
            .as_ref()
            .filter(|store| store.seed == seed)
            .map_or((0, 0.0), |store| {
                (store.site_count() as u64, store.total_upkeep_per_day())
            })
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
        // Read the site counts and upkeep totals before taking the economy lock:
        // the lock order is world -> ... -> economy -> utilities/services.
        let seed = {
            let world = self.world.lock().unwrap_or_else(|error| error.into_inner());
            world.as_ref().map(|store| store.seed)
        };
        let (utility_sites, utility_upkeep, service_sites, service_upkeep) =
            seed.map_or((0, 0.0, 0, 0.0), |seed| {
                let (utility_sites, utility_upkeep) = self.utility_infrastructure_for_seed(seed);
                let (service_sites, service_upkeep) = self.service_infrastructure_for_seed(seed);
                (utility_sites, utility_upkeep, service_sites, service_upkeep)
            });
        let infrastructure = match InfrastructureCosts::new(utility_upkeep, service_upkeep) {
            Ok(costs) => costs,
            Err(error) => {
                return placement_simple_error("invalid_infrastructure", error.to_string());
            }
        };
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
                result.set("debt", economy.debt());
                result.set("loan_count", economy.loans.len() as i64);
                result.set("loan_interest_per_day", economy.loan_interest_per_day());
                result.set("loan_payment_per_day", economy.loan_payment_per_day());
                result.set(
                    "can_borrow",
                    !economy.bankrupt
                        && economy.loans.len() < MAX_LOANS
                        && economy.debt() < MAX_TOTAL_DEBT,
                );
                result.set(
                    "daily_net",
                    economy.daily_net_with_infrastructure_costs(
                        population,
                        employed,
                        buildings,
                        infrastructure,
                    ),
                );
                result.set("infrastructure_per_day", infrastructure.total_per_day());
                result.set("utility_sites", utility_sites as i64);
                result.set("service_sites", service_sites as i64);
                result.set("utility_upkeep", infrastructure.utility_per_day());
                result.set("service_upkeep", infrastructure.service_per_day());
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
                result.set("infrastructure_per_day", 0.0);
                result.set("utility_sites", 0);
                result.set("service_sites", 0);
                result.set("utility_upkeep", 0.0);
                result.set("service_upkeep", 0.0);
                result.set("tax_per_resident_per_day", 0.0);
                result.set("business_tax_per_job_per_day", 0.0);
                result.set("upkeep_per_building_per_day", 0.0);
                result.set("construction_spent", 0.0);
                result.set("days_elapsed", 0.0);
                result.set("debt", 0.0);
                result.set("loan_count", 0);
                result.set("loan_interest_per_day", 0.0);
                result.set("loan_payment_per_day", 0.0);
                result.set("can_borrow", false);
                result.set("daily_net", 0.0);
                result.set("checksum", &GString::from("0000000000000000"));
            }
        }
        result
    }

    /// Pay any installment now due on operational utility sites.
    ///
    /// Callers hold the economy lock; the documented lock order is
    /// economy -> utilities/services. Unaffordable amounts stay due.
    fn settle_utility_payments(&self, seed: u64, economy: &mut Economy) -> f64 {
        let mut guard = self
            .utilities
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(utilities) = guard.as_mut() else {
            return 0.0;
        };
        if utilities.seed != seed {
            return 0.0;
        }
        let mut unpaid = 0.0;
        for kind in UtilityKind::ALL {
            let ids: Vec<u64> = match kind {
                UtilityKind::Water => utilities.water_sites.keys().copied().collect(),
                UtilityKind::Power => utilities.power_sites.keys().copied().collect(),
            };
            for id in ids {
                let due = utilities.payment_due(id, kind);
                if due <= 0.0 {
                    continue;
                }
                if economy.try_charge_construction(due) {
                    if let Some(site) = utilities.site_mut(id, kind) {
                        let _ = site.mark_paid(due);
                    }
                } else {
                    unpaid += due;
                }
            }
        }
        unpaid
    }

    /// Pay any installment now due on operational service sites.
    fn settle_service_payments(&self, seed: u64, economy: &mut Economy) -> f64 {
        let mut guard = self
            .services
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(services) = guard.as_mut() else {
            return 0.0;
        };
        if services.seed != seed {
            return 0.0;
        }
        let ids: Vec<u64> = services.sites.keys().copied().collect();
        let mut unpaid = 0.0;
        for id in ids {
            let due = services.payment_due(id);
            if due <= 0.0 {
                continue;
            }
            if economy.try_charge_construction(due) {
                if let Some(site) = services.site_mut(id) {
                    let _ = site.mark_paid(due);
                }
            } else {
                unpaid += due;
            }
        }
        unpaid
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
        // Utility and civic lead times run on the same simulated day clock as
        // the treasury. In developer mode every lead is already zero, so this
        // is a no-op and default checksums stay pinned.
        let days = (seconds / core_city::DAY_SECONDS) as f32;
        if let Err(error) = self.advance_infrastructure_lead(seed, days) {
            return placement_simple_error("invalid_delta", error);
        }
        let summary = self.city_summary();
        let population = summary
            .get("population")
            .map_or(0, |value| value.to::<i64>()) as u64;
        let employed = summary.get("employed").map_or(0, |value| value.to::<i64>()) as u64;
        let buildings = summary
            .get("completed_buildings")
            .map_or(0, |value| value.to::<i64>()) as u64;
        // Read the site counts and upkeep totals before taking the economy lock:
        // the lock order is world -> ... -> economy -> utilities/services.
        let (utility_sites, utility_upkeep) = self.utility_infrastructure_for_seed(seed);
        let (service_sites, service_upkeep) = self.service_infrastructure_for_seed(seed);
        let infrastructure = match InfrastructureCosts::new(utility_upkeep, service_upkeep) {
            Ok(costs) => costs,
            Err(error) => {
                return placement_simple_error("invalid_infrastructure", error.to_string());
            }
        };
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
        let tick = economy.tick_with_infrastructure_costs(
            seconds,
            population,
            employed,
            buildings,
            infrastructure,
        );
        let utility_payment_due = self.settle_utility_payments(seed, economy);
        let service_payment_due = self.settle_service_payments(seed, economy);
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("income", tick.income);
        result.set("expenses", tick.expenses);
        result.set("infrastructure", tick.infrastructure);
        result.set("utility_sites", utility_sites as i64);
        result.set("service_sites", service_sites as i64);
        result.set("utility_upkeep", infrastructure.utility_per_day());
        result.set("service_upkeep", infrastructure.service_per_day());
        result.set("balance", tick.balance);
        result.set("utility_payment_due", utility_payment_due);
        result.set("service_payment_due", service_payment_due);
        result.set(
            "infrastructure_payment_due",
            utility_payment_due + service_payment_due,
        );
        result.set("days_elapsed", tick.days_elapsed);
        result.set("loan_interest", tick.loan_interest);
        result.set("loan_principal", tick.loan_principal);
        result.set("debt", tick.debt);
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

    /// Borrow `amount` with the default rate and term.
    #[func]
    fn economy_borrow(&self, seed: i64, amount: f64) -> VarDictionary {
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
        {
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
            if let Err(error) =
                economy.take_loan(amount, DEFAULT_LOAN_RATE_PER_DAY, DEFAULT_LOAN_TERM_DAYS)
            {
                return placement_simple_error("loan_rejected", error.to_string());
            }
        }
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
        let power_supply = self
            .utilities
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .filter(|utilities| utilities.seed == seed)
            .map_or(0.0, |utilities| utilities.power_supply);
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
            result.set("power_supply", 0.0);
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
        let tick = industry.tick_with_power(seconds, &staffing, population, power_supply);
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("open", true);
        result.set("wood", tick.wood);
        result.set("food", tick.food);
        result.set("hides", tick.hides);
        result.set("food_consumed", tick.food_consumed);
        result.set("food_supply", f64::from(tick.food_supply));
        result.set("power_supply", f64::from(tick.power_supply));
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

    /// Designate a placement as a water or power site.
    #[func]
    fn utility_add_site(&self, seed: i64, placement_id: i64, kind: GString) -> VarDictionary {
        let Ok(seed) = u64::try_from(seed) else {
            return placement_simple_error("invalid_seed", "world seed must be non-negative");
        };
        let Ok(placement_id) = u64::try_from(placement_id) else {
            return placement_simple_error(
                "invalid_placement",
                "placement id must be non-negative",
            );
        };
        let Some(kind) = UtilityKind::parse(&kind.to_string()) else {
            return placement_simple_error(
                "invalid_utility",
                "utility kind must be water or power",
            );
        };
        let (center, time_scale) = {
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
                    "a utility site needs at least one job slot",
                );
            }
            let construction = self
                .construction
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if !construction
                .site(placement_id)
                .is_some_and(|site| site.is_complete())
            {
                return placement_simple_error(
                    "not_complete",
                    "a utility site needs a completed building",
                );
            }
            (placement.center, construction.time_scale())
        };
        let lead_days = construction_lead_days(time_scale, kind.campaign_build_days());
        let cost = kind.construction_cost();
        let initial_due = infrastructure_initial_due(cost, lead_days);
        let mut economy = self
            .economy
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let created = match economy_for_seed(&mut economy, seed) {
            Ok((treasury, created)) => {
                if !treasury.can_afford(initial_due) {
                    let balance = treasury.balance;
                    rollback_created_treasury(&mut economy, created);
                    return placement_simple_error(
                        "insufficient_funds",
                        format!(
                            "utility site needs {initial_due} now and the treasury has {balance}"
                        ),
                    );
                }
                created
            }
            Err((code, message)) => return placement_simple_error(code, message),
        };
        let cost_paid = {
            let mut guard = self
                .utilities
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if guard
                .as_ref()
                .is_some_and(|utilities| utilities.seed != seed)
            {
                rollback_created_treasury(&mut economy, created);
                return placement_simple_error(
                    "seed_mismatch",
                    "the utilities store belongs to a different seed",
                );
            }
            let utilities = guard.get_or_insert_with(|| UtilityStore::new(seed));
            if let Err(error) = utilities.add_priced_site_with_lead_days(
                placement_id,
                kind,
                center,
                lead_days,
                cost,
            ) {
                rollback_created_treasury(&mut economy, created);
                return placement_simple_error("utility_failed", error.to_string());
            }
            let due = utilities.payment_due(placement_id, kind);
            if let Err((code, message)) =
                charge_seed_treasury(&mut economy, created, due, "utility site")
            {
                utilities.remove_placement(placement_id);
                return placement_simple_error(code, message);
            }
            if let Some(site) = utilities.site_mut(placement_id, kind) {
                let _ = site.mark_paid(due);
            }
            utilities
                .site(placement_id, kind)
                .map_or(0.0, |site| site.cost_paid)
        };
        drop(economy);
        let payment = infrastructure_payment(cost, cost_paid);
        let mut result = self.utilities_refresh(seed as i64);
        result.set("placement_id", placement_id as i64);
        result.set("kind", &GString::from(kind.as_str()));
        result.set("construction_cost", cost);
        result.set("cost_total", payment.cost_total);
        result.set("cost_paid", payment.cost_paid);
        result.set("payment_due", payment.payment_due);
        result.set("charged", payment.charged);
        result
    }

    /// Remove one water or power site and refresh service.
    #[func]
    fn utility_remove_site(&self, seed: i64, placement_id: i64, kind: GString) -> VarDictionary {
        let Ok(seed) = u64::try_from(seed) else {
            return placement_simple_error("invalid_seed", "world seed must be non-negative");
        };
        let Ok(placement_id) = u64::try_from(placement_id) else {
            return placement_simple_error(
                "invalid_placement",
                "placement id must be non-negative",
            );
        };
        let Some(kind) = UtilityKind::parse(&kind.to_string()) else {
            return placement_simple_error(
                "invalid_utility",
                "utility kind must be water or power",
            );
        };
        {
            let mut guard = self
                .utilities
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(utilities) = guard.as_mut() {
                if utilities.seed != seed {
                    return placement_simple_error(
                        "seed_mismatch",
                        "the utilities store belongs to a different seed",
                    );
                }
                utilities.remove_site(placement_id, kind);
            }
        }
        self.utilities_refresh(seed as i64)
    }

    /// Return utility sites, coverage, demand, and service shares.
    #[func]
    fn utilities_state(&self) -> VarDictionary {
        let consumers = self.collect_utility_consumers();
        let mut guard = self
            .utilities
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match guard.as_mut() {
            Some(store) => {
                let report = store.refresh(&consumers);
                utility_payload(store, &report)
            }
            None => utility_closed_payload(consumers.len()),
        }
    }

    /// Recompute utility service from completed buildings and households.
    #[func]
    fn utilities_refresh(&self, seed: i64) -> VarDictionary {
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
        let consumers = self.collect_utility_consumers();
        let mut guard = self
            .utilities
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match guard.as_mut() {
            Some(store) => {
                if store.seed != seed {
                    return placement_simple_error(
                        "seed_mismatch",
                        "the utilities store belongs to a different seed",
                    );
                }
                let report = store.refresh(&consumers);
                utility_payload(store, &report)
            }
            None => utility_closed_payload(consumers.len()),
        }
    }

    /// Stable utilities checksum.
    #[func]
    fn utilities_checksum(&self) -> GString {
        let guard = self
            .utilities
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let checksum = guard.as_ref().map_or(0, UtilityStore::checksum);
        GString::from(&format!("{checksum:016x}"))
    }

    /// Close the utilities store.
    #[func]
    fn utilities_clear(&self) -> bool {
        *self
            .utilities
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        true
    }

    /// Add or replace a civic service site on a completed job building.
    ///
    /// The tier is derived from the city's current age: Wood is basic,
    /// Masonry/Industrial improved, Modern/Information advanced.
    #[func]
    fn service_add_site(&self, seed: i64, placement_id: i64, kind: GString) -> VarDictionary {
        let Ok(seed) = u64::try_from(seed) else {
            return placement_simple_error("invalid_seed", "world seed must be non-negative");
        };
        let Ok(placement_id) = u64::try_from(placement_id) else {
            return placement_simple_error(
                "invalid_placement",
                "placement id must be non-negative",
            );
        };
        let Some(kind) = ServiceKind::parse(&kind.to_string()) else {
            return placement_simple_error(
                "invalid_service",
                "service kind must be school, clinic, fire, or police",
            );
        };
        let city = self.city_summary();
        let age = AgeState::from_population(variant_u64(&city, "population")).age;
        let tier = ServiceTier::from_age(age);
        let (center, time_scale) = {
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
                    "a civic service site needs at least one job slot",
                );
            }
            let construction = self
                .construction
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if !construction
                .site(placement_id)
                .is_some_and(|site| site.is_complete())
            {
                return placement_simple_error(
                    "not_complete",
                    "a civic service site needs a completed building",
                );
            }
            (placement.center, construction.time_scale())
        };
        let lead_days = construction_lead_days(time_scale, kind.campaign_build_days(tier));
        let cost = kind.construction_cost(tier);
        let initial_due = infrastructure_initial_due(cost, lead_days);
        let mut economy = self
            .economy
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let created = match economy_for_seed(&mut economy, seed) {
            Ok((treasury, created)) => {
                if !treasury.can_afford(initial_due) {
                    let balance = treasury.balance;
                    rollback_created_treasury(&mut economy, created);
                    return placement_simple_error(
                        "insufficient_funds",
                        format!(
                            "service site needs {initial_due} now and the treasury has {balance}"
                        ),
                    );
                }
                created
            }
            Err((code, message)) => return placement_simple_error(code, message),
        };
        let cost_paid = {
            let mut guard = self
                .services
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if guard.as_ref().is_some_and(|services| services.seed != seed) {
                rollback_created_treasury(&mut economy, created);
                return placement_simple_error(
                    "seed_mismatch",
                    "the services store belongs to a different seed",
                );
            }
            let services = guard.get_or_insert_with(|| ServiceStore::new(seed));
            if let Err(error) = services.add_priced_site_with_lead_days(
                placement_id,
                kind,
                tier,
                center,
                lead_days,
                cost,
            ) {
                rollback_created_treasury(&mut economy, created);
                return placement_simple_error("service_failed", error.to_string());
            }
            let due = services.payment_due(placement_id);
            if let Err((code, message)) =
                charge_seed_treasury(&mut economy, created, due, "service site")
            {
                services.remove_placement(placement_id);
                return placement_simple_error(code, message);
            }
            if let Some(site) = services.site_mut(placement_id) {
                let _ = site.mark_paid(due);
            }
            services
                .site(placement_id)
                .map_or(0.0, |site| site.cost_paid)
        };
        drop(economy);
        let payment = infrastructure_payment(cost, cost_paid);
        let mut result = self.services_refresh(seed as i64);
        result.set("placement_id", placement_id as i64);
        result.set("kind", &GString::from(kind.as_str()));
        result.set("tier", &GString::from(tier.as_str()));
        result.set("construction_cost", cost);
        result.set("cost_total", payment.cost_total);
        result.set("cost_paid", payment.cost_paid);
        result.set("payment_due", payment.payment_due);
        result.set("charged", payment.charged);
        result
    }

    /// Remove a civic service site and refresh service.
    #[func]
    fn service_remove_site(&self, seed: i64, placement_id: i64) -> VarDictionary {
        let Ok(seed) = u64::try_from(seed) else {
            return placement_simple_error("invalid_seed", "world seed must be non-negative");
        };
        let Ok(placement_id) = u64::try_from(placement_id) else {
            return placement_simple_error(
                "invalid_placement",
                "placement id must be non-negative",
            );
        };
        {
            let mut guard = self
                .services
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(services) = guard.as_mut() {
                if services.seed != seed {
                    return placement_simple_error(
                        "seed_mismatch",
                        "the services store belongs to a different seed",
                    );
                }
                services.remove_site(placement_id);
            }
        }
        self.services_refresh(seed as i64)
    }

    /// Return civic service sites, coverage, demand, and research output.
    #[func]
    fn services_state(&self) -> VarDictionary {
        let consumers = self.collect_service_consumers();
        let mut guard = self
            .services
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match guard.as_mut() {
            Some(store) => {
                let report = store.refresh(&consumers);
                service_payload(store, &report)
            }
            None => service_closed_payload(consumers.len()),
        }
    }

    /// Recompute civic service from completed buildings and households.
    #[func]
    fn services_refresh(&self, seed: i64) -> VarDictionary {
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
        let consumers = self.collect_service_consumers();
        let mut guard = self
            .services
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match guard.as_mut() {
            Some(store) => {
                if store.seed != seed {
                    return placement_simple_error(
                        "seed_mismatch",
                        "the services store belongs to a different seed",
                    );
                }
                let report = store.refresh(&consumers);
                service_payload(store, &report)
            }
            None => service_closed_payload(consumers.len()),
        }
    }

    /// Stable civic service checksum.
    #[func]
    fn services_checksum(&self) -> GString {
        let guard = self
            .services
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let checksum = guard.as_ref().map_or(0, ServiceStore::checksum);
        GString::from(&format!("{checksum:016x}"))
    }

    /// Close the civic service store.
    #[func]
    fn services_clear(&self) -> bool {
        *self
            .services
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        true
    }

    fn collect_service_consumers(&self) -> Vec<ServiceConsumer> {
        let world = self.world.lock().unwrap_or_else(|error| error.into_inner());
        let construction = self
            .construction
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let population = self
            .population
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let runtime = population.as_ref().filter(|runtime| {
            world
                .as_ref()
                .is_some_and(|store| store.seed == runtime.seed)
        });
        completed_service_consumers(world.as_ref(), &construction, runtime)
    }

    fn civic_research_per_day(&self, seed: u64) -> f64 {
        let consumers = self.collect_service_consumers();
        let mut guard = self
            .services
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match guard.as_mut() {
            Some(store) if store.seed == seed => store.refresh(&consumers).research_per_day,
            _ => 0.0,
        }
    }

    fn collect_utility_consumers(&self) -> Vec<UtilityConsumer> {
        let world = self.world.lock().unwrap_or_else(|error| error.into_inner());
        let construction = self
            .construction
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let population = self
            .population
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let runtime = population.as_ref().filter(|runtime| {
            world
                .as_ref()
                .is_some_and(|store| store.seed == runtime.seed)
        });
        completed_utility_consumers(world.as_ref(), &construction, runtime)
    }

    /// Deterministic dashboard advice from the current city snapshot.
    #[func]
    fn city_advisor(&self, seed: i64) -> VarDictionary {
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
        let city = self.city_summary();
        let utilities = self.utilities_state();
        let services = self.services_state();
        let industry = self.industry_state();
        let economy = self.economy_state();
        let traffic = self.traffic_state(seed as i64);
        let construction = self.construction_summary();
        let mut input = AdvisorInput {
            population: variant_u64(&city, "population"),
            housing_capacity: variant_u64(&city, "housing_capacity"),
            vacancies: variant_u64(&city, "vacancies"),
            jobs: variant_u64(&city, "jobs"),
            employed: variant_u64(&city, "employed"),
            unemployed: variant_u64(&city, "unemployed"),
            happiness: variant_f32(&city, "happiness"),
            food: variant_f32(&city, "food"),
            water: variant_f32(&city, "water"),
            rest: variant_f32(&city, "rest"),
            average_commute_m: variant_f32(&city, "average_commute_m"),
            active_sites: variant_usize(&construction, "active"),
            stalled_sites: variant_usize(&construction, "stalled"),
            completed_buildings: variant_usize(&city, "completed_buildings"),
            ..AdvisorInput::default()
        };
        if variant_bool(&utilities, "open") {
            input.water_supply = Some(variant_f32(&utilities, "water_supply"));
            input.power_supply = Some(variant_f32(&utilities, "power_supply"));
        }
        if variant_bool(&services, "open") {
            input.school_supply = Some(variant_f32(&services, "school_supply"));
            input.clinic_supply = Some(variant_f32(&services, "clinic_supply"));
            input.fire_supply = Some(variant_f32(&services, "fire_supply"));
            input.police_supply = Some(variant_f32(&services, "police_supply"));
        }
        if variant_bool(&industry, "open") {
            input.food_supply = Some(variant_f32(&industry, "food_supply"));
            input.food_stock = variant_f64(&industry, "food");
            input.wood_stock = variant_f64(&industry, "wood");
        }
        if variant_bool(&economy, "open") {
            input.balance = Some(variant_f64(&economy, "balance"));
            input.daily_net = Some(variant_f64(&economy, "daily_net"));
            input.debt = Some(variant_f64(&economy, "debt"));
            input.bankrupt = variant_bool(&economy, "bankrupt");
        }
        if variant_bool(&traffic, "open") {
            input.traffic_components = Some(variant_usize(&traffic, "components"));
            input.traffic_nodes = Some(variant_usize(&traffic, "nodes"));
        } else {
            // A closed road store means the city has no roads to route over.
            input.traffic_components = Some(0);
            input.traffic_nodes = Some(0);
        }
        let report = advise(&input);
        advisor_payload(seed, &report)
    }

    /// Return the research pool, technology tree, and node availability.
    #[func]
    fn research_state(&self, seed: i64) -> VarDictionary {
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
        let city = self.city_summary();
        let population = variant_u64(&city, "population");
        let age = AgeState::from_population(population).age;
        let rate = research_rate_per_day(population, self.civic_research_per_day(seed));
        let guard = self
            .research
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if guard.as_ref().is_some_and(|store| store.seed != seed) {
            return placement_simple_error(
                "seed_mismatch",
                "the research pool belongs to a different seed",
            );
        }
        let fallback = ResearchStore::new(seed);
        let (store, open) = match guard.as_ref() {
            Some(store) => (store, true),
            None => (&fallback, false),
        };
        research_payload(store, default_tech_tree(), age, population, rate, open)
    }

    /// Advance the research pool by `seconds` at the current population rate.
    #[func]
    fn research_tick(&self, seed: i64, seconds: f64) -> VarDictionary {
        let Ok(seed) = u64::try_from(seed) else {
            return placement_simple_error("invalid_seed", "world seed must be non-negative");
        };
        if !seconds.is_finite() || !(0.0..=core_city::MAX_RESEARCH_TICK_SECONDS).contains(&seconds)
        {
            return placement_simple_error("invalid_delta", "research delta is outside 0..=86400");
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
        let city = self.city_summary();
        let population = variant_u64(&city, "population");
        let age = AgeState::from_population(population).age;
        let rate = research_rate_per_day(population, self.civic_research_per_day(seed));
        let mut guard = self
            .research
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if guard.as_ref().is_some_and(|store| store.seed != seed) {
            return placement_simple_error(
                "seed_mismatch",
                "the research pool belongs to a different seed",
            );
        }
        let store = guard.get_or_insert_with(|| ResearchStore::new(seed));
        let tick = store.tick(seconds, rate);
        let mut result = research_payload(store, default_tech_tree(), age, population, rate, true);
        result.set("gained", tick.gained);
        result
    }

    /// Spend research points on one technology node.
    #[func]
    fn research_unlock(&self, seed: i64, node: GString) -> VarDictionary {
        let Ok(seed) = u64::try_from(seed) else {
            return placement_simple_error("invalid_seed", "world seed must be non-negative");
        };
        let node = node.to_string();
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
        let city = self.city_summary();
        let population = variant_u64(&city, "population");
        let age = AgeState::from_population(population).age;
        let rate = research_rate_per_day(population, self.civic_research_per_day(seed));
        let mut guard = self
            .research
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if guard.as_ref().is_some_and(|store| store.seed != seed) {
            return placement_simple_error(
                "seed_mismatch",
                "the research pool belongs to a different seed",
            );
        }
        let store = guard.get_or_insert_with(|| ResearchStore::new(seed));
        match store.unlock(&node, default_tech_tree(), age) {
            Ok(unlock) => {
                let mut result =
                    research_payload(store, default_tech_tree(), age, population, rate, true);
                result.set("spent", unlock.spent);
                result.set("node", &GString::from(unlock.node.id.as_str()));
                result.set("name", &GString::from(unlock.node.name.as_str()));
                result
            }
            Err(error) => placement_simple_error(error.code(), error.to_string()),
        }
    }

    /// Stable research-pool checksum.
    #[func]
    fn research_checksum(&self) -> GString {
        let guard = self
            .research
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let checksum = guard.as_ref().map_or(0, ResearchStore::checksum);
        GString::from(&format!("{checksum:016x}"))
    }

    /// Close and clear the research pool.
    #[func]
    fn research_clear(&self) -> bool {
        *self
            .research
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        true
    }

    /// Derived population-age state for the current city.
    ///
    /// Age is not saved: it is a pure function of the completed city
    /// population. Research and explicit tech unlocks arrive in a later slice.
    #[func]
    fn progression_state(&self, seed: i64) -> VarDictionary {
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
        let city = self.city_summary();
        let population = variant_u64(&city, "population");
        let state = AgeState::from_population(population);
        let mut materials: Array<GString> = Array::new();
        for material in state.age.materials() {
            materials.push(&GString::from(*material));
        }
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("seed", seed as i64);
        result.set("population", population as i64);
        result.set("age", state.age.name());
        result.set(
            "age_index",
            Age::ALL
                .iter()
                .position(|age| *age == state.age)
                .map_or(0, |index| index as i64),
        );
        result.set("next_age", state.next_age.map_or("", |age| age.name()));
        result.set("next_threshold", state.next_threshold.unwrap_or(0) as i64);
        result.set("progress", f64::from(state.progress));
        result.set("final_age", state.is_final());
        result.set("materials", &materials.to_variant());
        result
    }

    /// Return the starter content catalog for the current age.
    #[func]
    fn starter_library(&self, seed: i64) -> VarDictionary {
        let Ok(seed) = u64::try_from(seed) else {
            return placement_simple_error("invalid_seed", "world seed must be non-negative");
        };
        if let Some(error) = self.world_seed_error(seed) {
            return error;
        }
        let age = AgeState::from_population(variant_u64(&self.city_summary(), "population")).age;
        let mut items: Array<VarDictionary> = Array::new();
        for building in core_city::starters() {
            let mut item = VarDictionary::new();
            item.set("id", &GString::from(building.id));
            item.set("name", &GString::from(building.name));
            item.set("category", &GString::from(building.category_str()));
            item.set("age", &GString::from(building.age.name()));
            item.set(
                "system",
                &GString::from(building.system.map_or("unchecked", StructuralSystem::id)),
            );
            item.set("stories", i64::from(building.stories));
            item.set(
                "footprint",
                &PackedFloat32Array::from(building.footprint_m.to_vec()).to_variant(),
            );
            item.set("resident_capacity", i64::from(building.resident_capacity));
            item.set("job_slots", i64::from(building.job_slots));
            item.set("material_cost", building.material_cost);
            item.set("available", building.available_at(age));
            items.push(&item);
        }
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("seed", seed as i64);
        result.set("age", &GString::from(age.name()));
        result.set("count", items.len() as i64);
        result.set("items", &items.to_variant());
        result
    }

    /// Generate (or reuse) a starter `.tsbp` and return its path.
    #[func]
    fn starter_blueprint(&self, seed: i64, id: GString) -> VarDictionary {
        let Ok(seed) = u64::try_from(seed) else {
            return placement_simple_error("invalid_seed", "world seed must be non-negative");
        };
        if let Some(error) = self.world_seed_error(seed) {
            return error;
        }
        let id = id.to_string();
        let Some(building) = starter(&id) else {
            return placement_simple_error("unknown_starter", format!("unknown starter {id}"));
        };
        let age = AgeState::from_population(variant_u64(&self.city_summary(), "population")).age;
        if !building.available_at(age) {
            return placement_simple_error(
                "age_locked",
                format!(
                    "{} requires the {} age; the city is {}",
                    building.name,
                    building.age.name(),
                    age.name()
                ),
            );
        }
        let path = match starter_package_path(building) {
            Ok(path) => path,
            Err(error) => return placement_simple_error("starter_failed", error),
        };
        if !path.exists() {
            let blueprint = match build_starter_blueprint(building) {
                Ok(blueprint) => blueprint,
                Err(error) => return placement_simple_error("starter_failed", error),
            };
            if let Err(error) = save_tsbp_with_hash(&path, &blueprint) {
                return placement_simple_error("starter_failed", error.to_string());
            }
        }
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("path", &GString::from(path.to_string_lossy().as_ref()));
        result.set("id", &GString::from(building.id));
        result.set("name", &GString::from(building.name));
        result.set("age", &GString::from(building.age.name()));
        result.set("stories", i64::from(building.stories));
        result.set("resident_capacity", i64::from(building.resident_capacity));
        result.set("job_slots", i64::from(building.job_slots));
        result.set("material_cost", building.material_cost);
        result
    }

    /// Derive the age-specific trade roster and install it on the store.
    #[func]
    fn construction_set_trades(&self, seed: i64) -> VarDictionary {
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
        let population = self
            .city_summary()
            .get("population")
            .map_or(0, |value| value.to::<i64>()) as u64;
        let age = AgeState::from_population(population).age;
        self.construction
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .set_trades(Some(TradePool::for_age(age)));
        self.construction_summary()
    }

    /// Derived tutorial checklist from the current city.
    ///
    /// This is not saved. It reads the existing industry, utility, service,
    /// placement, and construction state, so a sandbox save can also be used
    /// as a tutorial.
    #[func]
    fn tutorial_state(&self, seed: i64) -> VarDictionary {
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
        let houses = {
            let world = self.world.lock().unwrap_or_else(|error| error.into_inner());
            let construction = self
                .construction
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            world.as_ref().map_or(0, |store| {
                completed_slots(store, &construction).len() as u64
            })
        };
        let (lumberjacks, farms) = {
            let industry = self
                .industry
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            match industry.as_ref().filter(|store| store.seed == seed) {
                Some(store) => (
                    store
                        .sites
                        .values()
                        .filter(|site| site.kind == IndustryKind::Lumberjack)
                        .count() as u64,
                    store
                        .sites
                        .values()
                        .filter(|site| site.kind == IndustryKind::Farm)
                        .count() as u64,
                ),
                None => (0, 0),
            }
        };
        let wells = {
            let utilities = self
                .utilities
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            utilities
                .as_ref()
                .filter(|store| store.seed == seed)
                .map_or(0, |store| store.count_of(UtilityKind::Water) as u64)
        };
        let schools = {
            let services = self
                .services
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            services
                .as_ref()
                .filter(|store| store.seed == seed)
                .map_or(0, |store| store.count_of(ServiceKind::School) as u64)
        };
        let report = evaluate_tutorial(TutorialInput {
            lumberjacks,
            farms,
            wells,
            houses,
            schools,
        });
        let mut steps: Array<VarDictionary> = Array::new();
        for step in &report.steps {
            let mut item = VarDictionary::new();
            item.set("id", &GString::from(step.step.as_str()));
            item.set("title", &GString::from(step.step.title()));
            item.set("hint", &GString::from(step.step.hint()));
            item.set("done", step.done);
            item.set("current", step.current);
            steps.push(&item);
        }
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("seed", seed as i64);
        result.set("completed", report.completed as i64);
        result.set("total", report.total as i64);
        result.set("complete", report.complete);
        result.set(
            "current",
            &GString::from(report.current.map_or("", |step| step.as_str())),
        );
        result.set(
            "current_title",
            &GString::from(report.current.map_or("", |step| step.title())),
        );
        result.set("steps", &steps.to_variant());
        result
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

    /// Select the construction clock.
    ///
    /// `mode` is `developer` (the fast default, rate ignored) or `campaign`
    /// (phase targets in game days, rate in 10..=3600 real seconds per game
    /// day). Returns `{ ok, mode, seconds_per_game_day, checksum }`; mode is
    /// the canonical `ConstructionTimeScale::name()` and the checksum covers
    /// only the clock state, not the sites.
    #[func]
    fn set_construction_time_scale(
        &self,
        mode: String,
        seconds_per_game_day: f64,
    ) -> VarDictionary {
        let scale = match parse_construction_time_scale(&mode, seconds_per_game_day) {
            Ok(scale) => scale,
            Err((code, message)) => return placement_simple_error(code, message),
        };
        let mut store = self
            .construction
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Err(error) = store.set_time_scale(scale) {
            return placement_simple_error("invalid_time_scale", error.to_string());
        }
        construction_time_scale_dictionary(store.time_scale())
    }

    /// Current construction clock state.
    #[func]
    fn construction_time_scale_state(&self) -> VarDictionary {
        let scale = self
            .construction
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .time_scale();
        construction_time_scale_dictionary(scale)
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
        let (
            store,
            construction,
            roads,
            lots,
            settlements,
            mut population,
            named,
            economy,
            mut utilities,
            mut services,
            industry,
            research,
            policies,
        ) = {
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
            let named = self.named.lock().unwrap_or_else(|error| error.into_inner());
            let economy = self
                .economy
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let utilities = self
                .utilities
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let services = self
                .services
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let industry = self
                .industry
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let research = self
                .research
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let policies = self
                .policies
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            (
                world.as_ref().cloned(),
                construction.clone(),
                roads.as_ref().cloned(),
                lots.as_ref().cloned(),
                settlements.as_ref().cloned(),
                population.as_ref().cloned(),
                named.as_ref().cloned(),
                economy.as_ref().cloned(),
                utilities.as_ref().cloned(),
                services.as_ref().cloned(),
                industry.as_ref().cloned(),
                research.as_ref().cloned(),
                policies.as_ref().cloned(),
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
        let named = match named_pool_for_save(named, population.as_ref(), store.seed) {
            Ok(named) => named,
            Err((code, message)) => return placement_simple_error(code, message),
        };
        if let Some(utilities) = utilities.as_mut() {
            utilities.water_sites.retain(|id, site| {
                let Some(placement) = store.placements.iter().find(|item| item.id == *id) else {
                    return false;
                };
                placement.job_slots > 0
                    && construction
                        .site(*id)
                        .is_some_and(|site| site.is_complete())
                    && (site.center[0] - placement.center[0]).abs() <= 0.001
                    && (site.center[1] - placement.center[1]).abs() <= 0.001
                    && (site.center[2] - placement.center[2]).abs() <= 0.001
            });
            utilities.power_sites.retain(|id, site| {
                let Some(placement) = store.placements.iter().find(|item| item.id == *id) else {
                    return false;
                };
                placement.job_slots > 0
                    && construction
                        .site(*id)
                        .is_some_and(|site| site.is_complete())
                    && (site.center[0] - placement.center[0]).abs() <= 0.001
                    && (site.center[1] - placement.center[1]).abs() <= 0.001
                    && (site.center[2] - placement.center[2]).abs() <= 0.001
            });
            let consumers =
                completed_utility_consumers(Some(&store), &construction, population.as_ref());
            utilities.refresh(&consumers);
            if let Err(error) = utilities.validate() {
                return placement_simple_error("invalid_utilities", error.to_string());
            }
        }
        if let Some(services) = services.as_mut() {
            services.sites.retain(|id, site| {
                let Some(placement) = store.placements.iter().find(|item| item.id == *id) else {
                    return false;
                };
                placement.job_slots > 0
                    && construction
                        .site(*id)
                        .is_some_and(|site| site.is_complete())
                    && (site.center[0] - placement.center[0]).abs() <= 0.001
                    && (site.center[1] - placement.center[1]).abs() <= 0.001
                    && (site.center[2] - placement.center[2]).abs() <= 0.001
            });
            let consumers =
                completed_service_consumers(Some(&store), &construction, population.as_ref());
            services.refresh(&consumers);
            if let Err(error) = services.validate() {
                return placement_simple_error("invalid_services", error.to_string());
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
        let json = match named.as_ref() {
            Some(named) => match attach_named_citizens(&json, &named.to_save()) {
                Ok(json) => json,
                Err(error) => return placement_simple_error("save_failed", error),
            },
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
        let json = match utilities.as_ref() {
            Some(utilities) => {
                if utilities.seed != store.seed {
                    return placement_simple_error(
                        "seed_mismatch",
                        "the utilities store belongs to a different seed",
                    );
                }
                match attach_utilities(&json, utilities) {
                    Ok(json) => json,
                    Err(error) => return placement_simple_error("save_failed", error),
                }
            }
            None => json,
        };
        let json = match services.as_ref() {
            Some(services) => {
                if services.seed != store.seed {
                    return placement_simple_error(
                        "seed_mismatch",
                        "the services store belongs to a different seed",
                    );
                }
                match attach_services(&json, services) {
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
        let json = match research.as_ref() {
            Some(research) => {
                if research.seed != store.seed {
                    return placement_simple_error(
                        "seed_mismatch",
                        "the research pool belongs to a different seed",
                    );
                }
                if let Err(error) = research.validate(default_tech_tree()) {
                    return placement_simple_error("invalid_research", error.to_string());
                }
                match attach_research(&json, research) {
                    Ok(json) => json,
                    Err(error) => return placement_simple_error("save_failed", error),
                }
            }
            None => json,
        };
        let json = match attach_optional_policies(json, policies.as_ref(), store.seed) {
            Ok(json) => json,
            Err((code, error)) => return placement_simple_error(code, error),
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
        let backup_rotated = match write_text_atomic(&path, &json) {
            Ok(rotated) => rotated,
            Err(error) => return placement_simple_error("save_failed", error.to_string()),
        };
        // The refreshed clones are committed only after the save is durable.
        // Sanitization drops stale sites, so a failed save must not mutate the
        // live stores.
        if let Some(utilities) = utilities.as_ref() {
            let mut guard = self
                .utilities
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if guard
                .as_ref()
                .is_some_and(|live| live.seed == utilities.seed)
            {
                *guard = Some(utilities.clone());
            }
        }
        if let Some(services) = services.as_ref() {
            let mut guard = self
                .services
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if guard
                .as_ref()
                .is_some_and(|live| live.seed == services.seed)
            {
                *guard = Some(services.clone());
            }
        }
        if let Some(named) = named.as_ref() {
            let mut guard = self.named.lock().unwrap_or_else(|error| error.into_inner());
            if guard.as_ref().is_none_or(|live| live.seed == named.seed) {
                *guard = Some(named.clone());
            }
        }
        if let Some(population) = population.as_ref() {
            let mut guard = self
                .population
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if guard
                .as_ref()
                .is_none_or(|live| live.seed == population.seed)
            {
                *guard = Some(population.clone());
            }
        }
        if let Some(policies) = policies.as_ref() {
            let mut guard = self
                .policies
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if guard
                .as_ref()
                .is_some_and(|live| live.seed == policies.seed)
            {
                *guard = Some(policies.clone());
            }
        }
        let (policy_count, policy_checksum) = policy_summary(policies.as_ref());
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("path", &GString::from(path.to_string_lossy().as_ref()));
        result.set("bytes", json.len() as i64);
        result.set("backup_rotated", backup_rotated);
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
        result.set("named", named.as_ref().map_or(0, NamedCitizens::len) as i64);
        result.set(
            "named_max",
            named.as_ref().map_or(0, |pool| pool.max) as i64,
        );
        result.set(
            "named_checksum",
            &GString::from(&format!(
                "{:016x}",
                named.as_ref().map_or(0, NamedCitizens::checksum)
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
            "utility_sites",
            utilities.as_ref().map_or(0, UtilityStore::site_count) as i64,
        );
        result.set(
            "utility_checksum",
            &GString::from(&format!(
                "{:016x}",
                utilities.as_ref().map_or(0, UtilityStore::checksum)
            )),
        );
        result.set(
            "service_sites",
            services.as_ref().map_or(0, ServiceStore::site_count) as i64,
        );
        result.set(
            "service_checksum",
            &GString::from(&format!(
                "{:016x}",
                services.as_ref().map_or(0, ServiceStore::checksum)
            )),
        );
        result.set(
            "research_points",
            research.as_ref().map_or(0.0, |store| store.points),
        );
        result.set(
            "research_completed",
            research.as_ref().map_or(0, ResearchStore::completed_count) as i64,
        );
        result.set(
            "research_checksum",
            &GString::from(&format!(
                "{:016x}",
                research.as_ref().map_or(0, ResearchStore::checksum)
            )),
        );
        result.set(
            "district_policies",
            policy_count as i64,
        );
        result.set(
            "district_policy_checksum",
            &GString::from(&format!("{policy_checksum:016x}")),
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
        let read = match read_json_with_backup(&path) {
            Ok(read) => read,
            Err(error) => return placement_simple_error("corrupt_save", error),
        };
        let json = read.json;
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
        let mut named = match named_from_world_json(&json, expected_seed) {
            Ok(Some(pool)) => pool,
            Ok(None) => NamedCitizens::new(expected_seed),
            Err((code, error)) => return placement_simple_error(code, error),
        };
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
        let utilities = match utilities_from_world_json(&json) {
            Ok(utilities) => utilities,
            Err((code, error)) => return placement_simple_error(code, error),
        };
        if let Some(utilities) = utilities.as_ref() {
            if utilities.seed != expected_seed {
                return placement_simple_error(
                    "seed_mismatch",
                    format!(
                        "the utilities store uses seed {} but this world is {expected_seed}",
                        utilities.seed
                    ),
                );
            }
        }
        let services = match services_from_world_json(&json) {
            Ok(services) => services,
            Err((code, error)) => return placement_simple_error(code, error),
        };
        if let Some(services) = services.as_ref() {
            if services.seed != expected_seed {
                return placement_simple_error(
                    "seed_mismatch",
                    format!(
                        "the services store uses seed {} but this world is {expected_seed}",
                        services.seed
                    ),
                );
            }
        }
        let research = match research_from_world_json(&json) {
            Ok(Some(research)) => research,
            Ok(None) => ResearchStore::new(expected_seed),
            Err((code, error)) => return placement_simple_error(code, error),
        };
        let research_points = research.points;
        let research_completed = research.completed_count();
        let research_checksum = research.checksum();
        if research.seed != expected_seed {
            return placement_simple_error(
                "seed_mismatch",
                format!(
                    "the research pool uses seed {} but this world is {expected_seed}",
                    research.seed
                ),
            );
        }
        let policies = match policies_from_world_json(&json, expected_seed) {
            Ok(policies) => policies,
            Err((code, error)) => return placement_simple_error(code, error),
        };
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
            let package_structure = structure_spec_for_blueprint(&package);
            if package.metadata.id != placement.blueprint_id
                || package.metadata.name != placement.name
                || package.metadata.bounds_m != placement.bounds_m
                || package.metadata.stats.resident_capacity != placement.resident_capacity
                || package.metadata.stats.job_slots != placement.job_slots
                || placement
                    .structure
                    .is_some_and(|structure| package_structure != Some(structure))
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
        if let Some(utilities) = utilities.as_ref() {
            for sites in [&utilities.water_sites, &utilities.power_sites] {
                for &id in sites.keys() {
                    let Some(placement) = store.placements.iter().find(|item| item.id == id) else {
                        return placement_simple_error(
                            "utility_orphan",
                            format!("utility site {id} has no placement"),
                        );
                    };
                    if placement.job_slots == 0 {
                        return placement_simple_error(
                            "utility_no_jobs",
                            format!("utility site {id} has no job slots"),
                        );
                    }
                    if !construction.site(id).is_some_and(|site| site.is_complete()) {
                        return placement_simple_error(
                            "utility_not_complete",
                            format!("utility site {id} is not complete"),
                        );
                    }
                    let site = sites.get(&id).expect("site");
                    if (site.center[0] - placement.center[0]).abs() > 0.001
                        || (site.center[1] - placement.center[1]).abs() > 0.001
                        || (site.center[2] - placement.center[2]).abs() > 0.001
                    {
                        return placement_simple_error(
                            "utility_center_mismatch",
                            format!("utility site {id} does not match its placement centre"),
                        );
                    }
                }
            }
        }
        if let Some(services) = services.as_ref() {
            for (&id, site) in &services.sites {
                let Some(placement) = store.placements.iter().find(|item| item.id == id) else {
                    return placement_simple_error(
                        "service_orphan",
                        format!("service site {id} has no placement"),
                    );
                };
                if placement.job_slots == 0 {
                    return placement_simple_error(
                        "service_no_jobs",
                        format!("service site {id} has no job slots"),
                    );
                }
                if !construction.site(id).is_some_and(|site| site.is_complete()) {
                    return placement_simple_error(
                        "service_not_complete",
                        format!("service site {id} is not complete"),
                    );
                }
                if (site.center[0] - placement.center[0]).abs() > 0.001
                    || (site.center[1] - placement.center[1]).abs() > 0.001
                    || (site.center[2] - placement.center[2]).abs() > 0.001
                {
                    return placement_simple_error(
                        "service_center_mismatch",
                        format!("service site {id} does not match its placement centre"),
                    );
                }
            }
        }
        population.sync(&completed_slots(&store, &construction));
        sanitize_staffing(&mut population, &store, &construction);
        if let Err(error) = population.validate() {
            return placement_simple_error("invalid_population", error.to_string());
        }
        named.sync(&population.households);
        if let Err(error) = named.validate() {
            return placement_simple_error("invalid_named_citizens", error.to_string());
        }
        let mut utilities = utilities;
        if let Some(utilities) = utilities.as_mut() {
            let consumers =
                completed_utility_consumers(Some(&store), &construction, Some(&population));
            utilities.refresh(&consumers);
            if let Err(error) = utilities.validate() {
                return placement_simple_error("invalid_utilities", error.to_string());
            }
        }
        let mut services = services;
        if let Some(services) = services.as_mut() {
            let consumers =
                completed_service_consumers(Some(&store), &construction, Some(&population));
            services.refresh(&consumers);
            if let Err(error) = services.validate() {
                return placement_simple_error("invalid_services", error.to_string());
            }
        }
        let population_count = population.population();
        let population_checksum = population.checksum();
        let named_count = named.len();
        let named_max = named.max;
        let named_checksum = named.checksum();
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
        let utility_count = utilities.as_ref().map_or(0, UtilityStore::site_count);
        let utility_checksum = utilities.as_ref().map_or(0, UtilityStore::checksum);
        let service_count = services.as_ref().map_or(0, ServiceStore::site_count);
        let service_checksum = services.as_ref().map_or(0, ServiceStore::checksum);
        let (policy_count, policy_checksum) = policy_summary(policies.as_ref());
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
        let mut named_guard = self.named.lock().unwrap_or_else(|error| error.into_inner());
        let mut economy_guard = self
            .economy
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut utilities_guard = self
            .utilities
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut services_guard = self
            .services
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut industry_guard = self
            .industry
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut research_guard = self
            .research
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut policies_guard = self
            .policies
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
        *named_guard = Some(named);
        *economy_guard = economy;
        *utilities_guard = utilities;
        *services_guard = services;
        *industry_guard = industry;
        *research_guard = Some(research);
        *policies_guard = policies;
        *deltas_guard = deltas;
        *self
            .terrain_seed
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            if deltas_count > 0 { Some(seed) } else { None };
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("path", &GString::from(path.to_string_lossy().as_ref()));
        result.set("recovered_from_backup", read.recovered_from_backup);
        result.set(
            "backup",
            &GString::from(
                read.backup
                    .as_ref()
                    .map_or_else(String::new, |path| path.to_string_lossy().to_string())
                    .as_str(),
            ),
        );
        result.set(
            "backup_checksum",
            &GString::from(&format!("{:016x}", read.backup_checksum)),
        );
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
        result.set("named", named_count as i64);
        result.set("named_max", named_max as i64);
        result.set(
            "named_checksum",
            &GString::from(&format!("{named_checksum:016x}")),
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
        result.set("utility_sites", utility_count as i64);
        result.set(
            "utility_checksum",
            &GString::from(&format!("{utility_checksum:016x}")),
        );
        result.set("service_sites", service_count as i64);
        result.set(
            "service_checksum",
            &GString::from(&format!("{service_checksum:016x}")),
        );
        result.set("research_points", research_points);
        result.set("research_completed", research_completed as i64);
        result.set(
            "research_checksum",
            &GString::from(&format!("{research_checksum:016x}")),
        );
        result.set("district_policies", policy_count as i64);
        result.set(
            "district_policy_checksum",
            &GString::from(&format!("{policy_checksum:016x}")),
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
    /// Installs the default acceptance volume even when no editor volume is
    /// open, replacing any current one.
    ///
    /// **Main thread only.**
    #[func]
    fn editor_build_acceptance_house(&self) -> bool {
        let mut guard = self
            .editor
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *guard = Some(acceptance_house_editor_state());
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

    /// Return the editor's current structural system and derived limits.
    #[func]
    fn editor_structure_state(&self) -> VarDictionary {
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
        let bounds_m = state.volume.config().bounds_m();
        let inputs = editor_props_input(&state.props);
        let stats = detect_rooms(&state.volume, &editor_room_materials(), &inputs);
        let stories = stats
            .rooms
            .iter()
            .map(|room| u16::try_from(room.floor.saturating_add(1)).unwrap_or(u16::MAX))
            .max()
            .unwrap_or_else(|| (bounds_m[1] / 3.2).ceil().max(1.0) as u16);
        let mut spec = StructureSpec::new(
            state.structure,
            stories,
            bounds_m[1],
            state.structure_span_m,
        );
        spec.ground_system = state.ground_system;
        if let Some(depth) = state.structure_foundation_m {
            spec.foundation_depth_m = depth;
        }
        let mut result = structure_payload(&spec);
        result.set("ok", true);
        result.set("created", true);
        result
    }

    /// Select the editor's structural system by id or alias.
    #[func]
    fn editor_set_structure(&self, kind: GString) -> VarDictionary {
        let Some(system) = StructuralSystem::parse(&kind.to_string()) else {
            let mut result = VarDictionary::new();
            result.set("ok", false);
            result.set("error", "unknown structural system");
            return result;
        };
        {
            let mut guard = self
                .editor
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let Some(state) = guard.as_mut() else {
                let mut result = VarDictionary::new();
                result.set("ok", false);
                result.set("error", "no editor volume is open");
                return result;
            };
            state.structure = system;
        }
        self.editor_structure_state()
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

        let (volume, props, structure, ground_system, structure_span_m, structure_foundation_m) = {
            let guard = self
                .editor
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let Some(state) = guard.as_ref() else {
                result.set("ok", false);
                result.set("error", "no editor volume is open");
                return result;
            };
            (
                state.volume.clone(),
                state.props.clone(),
                state.structure,
                state.ground_system,
                state.structure_span_m,
                state.structure_foundation_m,
            )
        };

        let blueprint = blueprint_from_editor_volume(
            id,
            name,
            volume,
            &props,
            structure,
            ground_system,
            structure_span_m,
            structure_foundation_m,
        );
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
        if let Some(structure) = blueprint.metadata.structure.as_ref() {
            result.set("structure", &structure_payload(structure).to_variant());
        }
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

        let loaded_spec = blueprint.metadata.structure;
        let loaded_structure =
            loaded_spec.map_or(StructuralSystem::TimberFrame, |structure| structure.system);
        let mut state = EditorState::new(blueprint.volume);
        state.structure = loaded_structure;
        state.ground_system = loaded_spec.and_then(|structure| structure.ground_system);
        state.structure_span_m = loaded_spec.map_or(0.0, |structure| structure.max_span_m);
        state.structure_foundation_m = loaded_spec.map(|structure| structure.foundation_depth_m);
        let structure_payload_value = loaded_spec.as_ref().map(structure_payload);
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
        if let Some(structure) = structure_payload_value {
            result.set("structure", &structure.to_variant());
        }
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

fn road_width_valid(width: i64) -> bool {
    (i64::from(core_city::MIN_ROAD_WIDTH_M)..=i64::from(core_city::MAX_ROAD_WIDTH_M))
        .contains(&width)
}

fn city_layout_seed(raw: i64) -> Result<u64, &'static str> {
    u64::try_from(raw).map_err(|_| "invalid_seed")
}

fn lot_store_seed_matches(store: Option<&LotStore>, seed: u64) -> bool {
    store.is_none_or(|store| store.seed == seed)
}

fn acceptance_house_editor_state() -> EditorState {
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
    state
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
    if !road_width_valid(width) {
        return Err(placement_simple_error(
            "invalid_width",
            format!(
                "road width must be {}..={} m",
                core_city::MIN_ROAD_WIDTH_M,
                core_city::MAX_ROAD_WIDTH_M
            ),
        ));
    }
    let Ok(width) = u8::try_from(width) else {
        return Err(placement_simple_error(
            "invalid_width",
            "road width is out of range",
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
    result.set(
        "structure",
        &placement
            .structure
            .as_ref()
            .map_or_else(VarDictionary::new, structure_payload)
            .to_variant(),
    );
    result
}

/// First construction installment (Survey) for a placement price.
fn first_construction_payment(material_cost: f64) -> f64 {
    if !material_cost.is_finite() || material_cost <= 0.0 {
        return 0.0;
    }
    (material_cost * ConstructionPhase::Survey.billing_weight() * 100.0).round() / 100.0
}

/// Charge the first construction installment and mark its site paid.
///
/// Returns `true` only when the treasury accepted the charge and the site
/// accepted the payment. A failed charge moves nothing; a site that rejects
/// the installment after the charge is refunded, so a rolled-back placement
/// cannot keep the money without a site (F-015).
fn settle_first_installment(
    economy: &mut Economy,
    construction: &mut ConstructionStore,
    placement_id: u64,
    amount: f64,
) -> bool {
    if !economy.try_charge_construction(amount) {
        return false;
    }
    let marked_paid = construction
        .site_mut(placement_id)
        .is_some_and(|site| site.mark_paid(amount).is_ok());
    if !marked_paid {
        economy.refund_construction(amount);
    }
    marked_paid
}

/// Lead time assigned to a new utility or civic site under a construction clock.
///
/// Developer mode keeps the legacy immediate behavior; campaign mode assigns
/// the kind/tier lead time in game days. Storing days rather than seconds keeps
/// the value independent of the campaign pacing rate.
fn construction_lead_days(scale: ConstructionTimeScale, days: f32) -> f32 {
    if scale.is_developer() {
        0.0
    } else {
        days
    }
}

/// First utility/service installment: half under a campaign lead time, the
/// full catalog cost on the developer clock.
fn infrastructure_initial_due(cost: f64, lead_days: f32) -> f64 {
    if lead_days > 0.0 {
        cost * 0.5
    } else {
        cost
    }
}

/// Construction payment figures reported after a successful infrastructure
/// add.
///
/// `cost_paid` is read from the site after the installment is recorded, so a
/// replacement reports the preserved progress rather than assuming a fresh
/// site. The bridge only builds this after the charge succeeded, so `charged`
/// is always true; the struct keeps the payload contract testable without a
/// `TownsmithCore`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct InfrastructurePayment {
    cost_total: f64,
    cost_paid: f64,
    payment_due: f64,
    charged: bool,
}

fn infrastructure_payment(cost_total: f64, cost_paid: f64) -> InfrastructurePayment {
    InfrastructurePayment {
        cost_total,
        cost_paid,
        payment_due: (cost_total - cost_paid).max(0.0),
        charged: true,
    }
}

/// Borrow the seed treasury, opening one with the starting balance when none
/// exists.
///
/// Returns the treasury and whether this call created it. A treasury for a
/// different seed is rejected like every other seed-taking bridge call.
/// Callers that fail after this point must call [`rollback_created_treasury`],
/// so a rejected infrastructure add leaves no treasury behind and cannot pin
/// the world to that seed (F-016).
fn economy_for_seed(
    economy: &mut Option<Economy>,
    seed: u64,
) -> Result<(&mut Economy, bool), (&'static str, String)> {
    if economy
        .as_ref()
        .is_some_and(|treasury| treasury.seed != seed)
    {
        return Err((
            "seed_mismatch",
            "the treasury belongs to a different seed".to_string(),
        ));
    }
    let created = economy.is_none();
    let treasury = economy.get_or_insert_with(|| Economy::new(seed));
    Ok((treasury, created))
}

/// Remove a treasury opened by the failed call; a pre-existing treasury and
/// its balance are left untouched.
fn rollback_created_treasury(economy: &mut Option<Economy>, created: bool) {
    if created {
        *economy = None;
    }
}

/// Charge an infrastructure installment, closing a treasury this call opened
/// when the treasury rejects it.
fn charge_seed_treasury(
    economy: &mut Option<Economy>,
    created: bool,
    due: f64,
    label: &str,
) -> Result<(), (&'static str, String)> {
    let Some(treasury) = economy.as_mut() else {
        return Err((
            "no_treasury",
            "no treasury is open for this seed".to_string(),
        ));
    };
    if !treasury.try_charge_construction(due) {
        rollback_created_treasury(economy, created);
        return Err((
            "insufficient_funds",
            format!("{label} needs {due} now and the treasury cannot cover it"),
        ));
    }
    Ok(())
}

/// Parse the bridge construction-clock mode.
///
/// `developer` restores the fast default and ignores the rate. `campaign`
/// requires a finite `seconds_per_game_day` in the core-supported
/// 10..=3600 range.
fn parse_construction_time_scale(
    mode: &str,
    seconds_per_game_day: f64,
) -> Result<ConstructionTimeScale, (&'static str, &'static str)> {
    match mode.trim() {
        "developer" => Ok(ConstructionTimeScale::DeveloperSeconds),
        "campaign" => ConstructionTimeScale::campaign(seconds_per_game_day).map_err(|_| {
            (
                "invalid_time_scale",
                "campaign seconds per game day must be finite and within 10..=3600",
            )
        }),
        _ => Err((
            "invalid_time_scale_mode",
            "construction time scale mode must be developer or campaign",
        )),
    }
}

/// Stable FNV-1a checksum of the clock state (mode plus campaign rate bits).
fn construction_time_scale_checksum(scale: ConstructionTimeScale) -> u64 {
    let mut payload = String::from(scale.name());
    if let Some(seconds_per_game_day) = scale.seconds_per_game_day() {
        payload.push(':');
        payload.push_str(&format!("{:016x}", seconds_per_game_day.to_bits()));
    }
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in payload.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Bridge payload for the construction clock state.
fn construction_time_scale_dictionary(scale: ConstructionTimeScale) -> VarDictionary {
    let mut result = VarDictionary::new();
    result.set("ok", true);
    result.set("mode", &GString::from(scale.name()));
    result.set(
        "seconds_per_game_day",
        scale.seconds_per_game_day().unwrap_or(0.0),
    );
    result.set(
        "checksum",
        &GString::from(&format!("{:016x}", construction_time_scale_checksum(scale))),
    );
    result
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct PaymentReport {
    paid_total: f64,
    paid_sites: u32,
    payment_stalled: u32,
}

/// Charge every due installment in ascending placement-id order.
///
/// The store keeps the schedule; the treasury is charged here. A failed
/// charge stalls the site at the next phase boundary and leaves it due for a
/// later retry. A missing treasury pays nothing and stalls nothing.
fn settle_construction_payments(
    store: &mut ConstructionStore,
    mut economy: Option<&mut Economy>,
) -> PaymentReport {
    let mut report = PaymentReport::default();
    let ids: Vec<u64> = store.sites.keys().copied().collect();
    for id in ids {
        let due = store.payment_due(id);
        if due <= 0.0 {
            continue;
        }
        let Some(economy) = economy.as_deref_mut() else {
            continue;
        };
        if economy.try_charge_construction(due) {
            if let Some(site) = store.site_mut(id) {
                if site.mark_paid(due).is_ok() {
                    site.resume_payment();
                    report.paid_total += due;
                    report.paid_sites += 1;
                }
            }
        } else if let Some(site) = store.site_mut(id) {
            site.stall_for_payment();
            report.payment_stalled += 1;
        }
    }
    report
}

fn construction_site_to_dictionary(site: &ConstructionSite, trade: Option<Trade>) -> VarDictionary {
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
    result.set("cost_total", site.cost_total);
    result.set("cost_paid", site.cost_paid);
    result.set("payment_due", site.payment_due());
    result.set("payment_stalled", site.payment_stalled);
    if let Some(trade) = trade {
        result.set("trade", &GString::from(trade.as_str()));
        result.set("trade_name", &GString::from(trade.name()));
    }
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
        structure: structure_spec_for_blueprint(blueprint),
    }
}

fn default_tech_tree() -> &'static TechTree {
    static TREE: OnceLock<TechTree> = OnceLock::new();
    TREE.get_or_init(TechTree::default_tree)
}

fn research_rate_per_day(population: u64, civic_research: f64) -> f64 {
    (population as f64 * RESEARCH_PER_RESIDENT_PER_DAY + civic_research.max(0.0))
        .min(core_city::MAX_RESEARCH_RATE_PER_DAY)
}

fn age_rank(age: Age) -> usize {
    Age::ALL
        .iter()
        .position(|candidate| *candidate == age)
        .unwrap_or(0)
}

fn structure_required_age(spec: &StructureSpec) -> (Age, StructuralSystem) {
    let mut required_system = spec.system;
    let mut required =
        Age::parse(spec.system.min_age()).expect("compiled structural systems have a known age");
    if let Some(ground) = spec.ground_system {
        let ground_age =
            Age::parse(ground.min_age()).expect("compiled structural systems have a known age");
        if age_rank(ground_age) > age_rank(required) {
            required = ground_age;
            required_system = ground;
        }
    }
    (required, required_system)
}

fn structure_age_error(spec: &StructureSpec, current: Age) -> Option<(&'static str, String)> {
    let (required, required_system) = structure_required_age(spec);
    if age_rank(current) >= age_rank(required) {
        return None;
    }
    Some((
        "age_locked",
        format!(
            "{} requires the {} age; the city is in the {} age",
            required_system.name(),
            required.name(),
            current.name()
        ),
    ))
}

fn research_payload(
    store: &ResearchStore,
    tree: &TechTree,
    age: Age,
    population: u64,
    rate_per_day: f64,
    open: bool,
) -> VarDictionary {
    let mut result = VarDictionary::new();
    result.set("ok", true);
    result.set("open", open);
    result.set("seed", store.seed as i64);
    result.set("age", age.name());
    result.set("age_index", age_rank(age) as i64);
    result.set("population", population as i64);
    result.set("points", store.points);
    result.set("lifetime_points", store.lifetime_points);
    result.set("rate_per_day", rate_per_day);
    result.set("completed_count", store.completed_count() as i64);
    result.set("node_count", tree.len() as i64);

    let mut completed: Array<GString> = Array::new();
    for id in &store.completed {
        completed.push(&GString::from(id.as_str()));
    }
    result.set("completed", &completed.to_variant());

    let mut nodes: Array<VarDictionary> = Array::new();
    for node in tree.ordered() {
        let unlocked = store.is_completed(&node.id);
        let age_locked = age < node.age;
        let prerequisites_met = node
            .prerequisites
            .iter()
            .all(|prerequisite| store.is_completed(prerequisite));
        let affordable = store.points + 0.000_001 >= node.cost;
        let available = store.can_unlock(node, age);
        let mut item = VarDictionary::new();
        item.set("id", &GString::from(node.id.as_str()));
        item.set("name", &GString::from(node.name.as_str()));
        item.set("age", node.age.name());
        item.set("cost", node.cost);
        let mut prerequisites: Array<GString> = Array::new();
        for prerequisite in &node.prerequisites {
            prerequisites.push(&GString::from(prerequisite.as_str()));
        }
        item.set("prerequisites", &prerequisites.to_variant());
        let mut unlocks: Array<GString> = Array::new();
        for unlock in &node.unlocks {
            unlocks.push(&GString::from(unlock.as_str()));
        }
        item.set("unlocks", &unlocks.to_variant());
        item.set("unlocked", unlocked);
        item.set("age_locked", age_locked);
        item.set("prerequisites_met", prerequisites_met);
        item.set("affordable", affordable);
        item.set("available", available);
        nodes.push(&item);
    }
    result.set("nodes", &nodes.to_variant());
    result.set(
        "checksum",
        &GString::from(&format!("{:016x}", store.checksum())),
    );
    result
}

/// Return the blueprint structure or derive a conservative timber frame.
fn structure_spec_for_blueprint(blueprint: &Blueprint) -> Option<StructureSpec> {
    if let Some(structure) = blueprint.metadata.structure {
        return Some(structure);
    }
    let bounds_m = blueprint.metadata.bounds_m;
    let fallback =
        StructureSpec::derive_from_bounds(StructuralSystem::TimberFrame, bounds_m).ok()?;
    let stories = blueprint
        .metadata
        .rooms
        .iter()
        .map(|room| u16::try_from(room.floor.saturating_add(1)).unwrap_or(u16::MAX))
        .max()
        .unwrap_or(fallback.stories);
    let derived = StructureSpec::new(
        StructuralSystem::TimberFrame,
        stories,
        fallback.height_m,
        0.0,
    );
    // Legacy packages stay placeable when the conservative derivation would
    // reject them; re-authoring is required to opt into structural rules.
    derived.validate().ok().map(|()| derived)
}

fn structure_payload(spec: &StructureSpec) -> VarDictionary {
    let mut result = VarDictionary::new();
    result.set("present", true);
    result.set("system", &GString::from(spec.system.id()));
    result.set("system_name", &GString::from(spec.system.name()));
    result.set("stories", i64::from(spec.stories));
    result.set("height_m", f64::from(spec.height_m));
    result.set("max_span_m", f64::from(spec.max_span_m));
    result.set("foundation_depth_m", f64::from(spec.foundation_depth_m));
    result.set(
        "required_foundation_m",
        f64::from(core_structure::recommended_foundation_m(spec.stories)),
    );
    result.set(
        "ground_system",
        &spec
            .ground_system
            .map_or_else(|| GString::from(""), |system| GString::from(system.id())),
    );
    match spec.summary() {
        Ok(summary) => {
            result.set("valid", true);
            result.set("error", &GString::from(""));
            result.set("error_code", &GString::from(""));
            result.set(
                "effective_max_stories",
                i64::from(summary.effective_max_stories),
            );
            result.set(
                "effective_max_height_m",
                f64::from(summary.effective_max_height_m),
            );
            result.set(
                "effective_max_span_m",
                f64::from(summary.effective_max_span_m),
            );
            result.set("composite_bonus", summary.composite_bonus);
        }
        Err(error) => {
            result.set("valid", false);
            result.set("error", &GString::from(error.to_string().as_str()));
            result.set("error_code", &GString::from(error.code()));
            result.set(
                "effective_max_stories",
                i64::from(spec.system.max_stories()),
            );
            result.set(
                "effective_max_height_m",
                f64::from(spec.system.max_height_m()),
            );
            result.set("effective_max_span_m", f64::from(spec.system.max_span_m()));
            result.set("composite_bonus", false);
        }
    }
    result
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

fn traffic_payload(graph: &RoadGraph, open: bool) -> VarDictionary {
    let stats = graph.stats();
    let mut result = VarDictionary::new();
    result.set("ok", true);
    result.set("open", open);
    result.set("roads", stats.roads as i64);
    result.set("nodes", stats.nodes as i64);
    result.set("edges", stats.edges as i64);
    result.set("components", stats.components as i64);
    result.set("connected", stats.connected());
    result.set("total_length_m", f64::from(stats.total_length_m));
    result.set(
        "checksum",
        &GString::from(&format!("{:016x}", graph.checksum())),
    );
    result
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

fn completed_utility_consumers(
    placements: Option<&PlacementStore>,
    construction: &ConstructionStore,
    population: Option<&CityRuntime>,
) -> Vec<UtilityConsumer> {
    let Some(placements) = placements else {
        return Vec::new();
    };
    placements
        .placements
        .iter()
        .filter(|placement| {
            construction
                .site(placement.id)
                .is_some_and(|site| site.is_complete())
        })
        .map(|placement| UtilityConsumer {
            id: placement.id,
            center: [placement.center[0], placement.center[2]],
            residents: population
                .and_then(|runtime| runtime.households.get(&placement.id))
                .map_or(0, |household| household.residents),
            workers: population
                .and_then(|runtime| runtime.staffing.get(&placement.id))
                .copied()
                .unwrap_or(0),
        })
        .collect()
}

fn completed_service_consumers(
    placements: Option<&PlacementStore>,
    construction: &ConstructionStore,
    population: Option<&CityRuntime>,
) -> Vec<ServiceConsumer> {
    let Some(placements) = placements else {
        return Vec::new();
    };
    placements
        .placements
        .iter()
        .filter(|placement| {
            construction
                .site(placement.id)
                .is_some_and(|site| site.is_complete())
        })
        .map(|placement| ServiceConsumer {
            id: placement.id,
            center: [placement.center[0], placement.center[2]],
            residents: population
                .and_then(|runtime| runtime.households.get(&placement.id))
                .map_or(0, |household| household.residents),
            workers: population
                .and_then(|runtime| runtime.staffing.get(&placement.id))
                .copied()
                .unwrap_or(0),
        })
        .collect()
}

fn service_closed_payload(consumers: usize) -> VarDictionary {
    let mut result = VarDictionary::new();
    result.set("ok", true);
    result.set("open", false);
    result.set("seed", 0);
    result.set("site_count", 0);
    result.set("ready", 0);
    result.set("building", 0);
    result.set("cost_total", 0.0);
    result.set("cost_paid", 0.0);
    result.set("payment_due", 0.0);
    result.set("school_sites", 0);
    result.set("clinic_sites", 0);
    result.set("fire_sites", 0);
    result.set("police_sites", 0);
    result.set("school_supply", 0.0);
    result.set("clinic_supply", 0.0);
    result.set("fire_supply", 0.0);
    result.set("police_supply", 0.0);
    result.set("school_demand", 0.0);
    result.set("clinic_demand", 0.0);
    result.set("fire_demand", 0.0);
    result.set("police_demand", 0.0);
    result.set("school_served", 0.0);
    result.set("clinic_served", 0.0);
    result.set("fire_served", 0.0);
    result.set("police_served", 0.0);
    result.set("school_capacity", 0.0);
    result.set("clinic_capacity", 0.0);
    result.set("fire_capacity", 0.0);
    result.set("police_capacity", 0.0);
    result.set("school_coverage", 0.0);
    result.set("clinic_coverage", 0.0);
    result.set("fire_coverage", 0.0);
    result.set("police_coverage", 0.0);
    result.set("research_per_day", 0.0);
    result.set("consumers", consumers as i64);
    result.set("sites", &Array::<VarDictionary>::new().to_variant());
    result.set("checksum", &GString::from("0000000000000000"));
    result
}

fn service_payload(store: &ServiceStore, report: &ServiceReport) -> VarDictionary {
    let mut result = VarDictionary::new();
    result.set("ok", true);
    result.set("open", true);
    result.set("seed", store.seed as i64);
    result.set("site_count", store.site_count() as i64);
    result.set("ready", store.ready_count() as i64);
    result.set("building", store.building_count() as i64);
    result.set(
        "cost_total",
        store.sites.values().map(|s| s.cost_total).sum::<f64>(),
    );
    result.set(
        "cost_paid",
        store.sites.values().map(|s| s.cost_paid).sum::<f64>(),
    );
    result.set("payment_due", store.total_payment_due());
    result.set("school_sites", store.count_of(ServiceKind::School) as i64);
    result.set("clinic_sites", store.count_of(ServiceKind::Clinic) as i64);
    result.set("fire_sites", store.count_of(ServiceKind::Fire) as i64);
    result.set("police_sites", store.count_of(ServiceKind::Police) as i64);
    result.set("school_supply", f64::from(report.school_supply));
    result.set("clinic_supply", f64::from(report.clinic_supply));
    result.set("fire_supply", f64::from(report.fire_supply));
    result.set("police_supply", f64::from(report.police_supply));
    result.set("school_demand", report.school_demand);
    result.set("clinic_demand", report.clinic_demand);
    result.set("fire_demand", report.fire_demand);
    result.set("police_demand", report.police_demand);
    result.set("school_served", report.school_served);
    result.set("clinic_served", report.clinic_served);
    result.set("fire_served", report.fire_served);
    result.set("police_served", report.police_served);
    result.set("school_capacity", report.school_capacity);
    result.set("clinic_capacity", report.clinic_capacity);
    result.set("fire_capacity", report.fire_capacity);
    result.set("police_capacity", report.police_capacity);
    result.set("school_coverage", f64::from(report.school_coverage));
    result.set("clinic_coverage", f64::from(report.clinic_coverage));
    result.set("fire_coverage", f64::from(report.fire_coverage));
    result.set("police_coverage", f64::from(report.police_coverage));
    result.set("research_per_day", report.research_per_day);
    result.set("consumers", report.consumers as i64);
    let mut sites: Array<VarDictionary> = Array::new();
    for (&id, site) in &store.sites {
        let mut item = VarDictionary::new();
        item.set("placement_id", id as i64);
        item.set("kind", &GString::from(site.kind.as_str()));
        item.set("kind_name", &GString::from(site.kind.name()));
        item.set("tier", &GString::from(site.tier.as_str()));
        item.set("radius_m", f64::from(site.radius_m));
        item.set("capacity", site.capacity);
        item.set("research_per_day", site.research_per_day);
        item.set("build_days_remaining", f64::from(site.build_days_remaining));
        item.set("cost_total", site.cost_total);
        item.set("cost_paid", site.cost_paid);
        item.set("payment_due", site.payment_due());
        item.set("center_x", f64::from(site.center[0]));
        item.set("center_y", f64::from(site.center[1]));
        item.set("center_z", f64::from(site.center[2]));
        sites.push(&item);
    }
    result.set("sites", &sites.to_variant());
    result.set(
        "checksum",
        &GString::from(&format!("{:016x}", store.checksum())),
    );
    result
}

fn utility_closed_payload(consumers: usize) -> VarDictionary {
    let mut result = VarDictionary::new();
    result.set("ok", true);
    result.set("open", false);
    result.set("seed", 0);
    result.set("site_count", 0);
    result.set("ready", 0);
    result.set("building", 0);
    result.set("cost_total", 0.0);
    result.set("cost_paid", 0.0);
    result.set("payment_due", 0.0);
    result.set("water_sites", 0);
    result.set("power_sites", 0);
    result.set("water_supply", 0.0);
    result.set("power_supply", 0.0);
    result.set("water_demand", 0.0);
    result.set("power_demand", 0.0);
    result.set("water_served", 0.0);
    result.set("power_served", 0.0);
    result.set("water_capacity", 0.0);
    result.set("power_capacity", 0.0);
    result.set("water_coverage", 0.0);
    result.set("power_coverage", 0.0);
    result.set("water_covered_buildings", 0);
    result.set("power_covered_buildings", 0);
    result.set("consumers", consumers as i64);
    result.set("sites", &Array::<VarDictionary>::new().to_variant());
    result.set("checksum", &GString::from("0000000000000000"));
    result
}

fn utility_payload(store: &UtilityStore, report: &UtilityReport) -> VarDictionary {
    let mut result = VarDictionary::new();
    result.set("ok", true);
    result.set("open", true);
    result.set("seed", store.seed as i64);
    result.set("site_count", store.site_count() as i64);
    result.set("ready", store.ready_count() as i64);
    result.set("building", store.building_count() as i64);
    result.set(
        "cost_total",
        store
            .water_sites
            .values()
            .map(|s| s.cost_total)
            .sum::<f64>()
            + store
                .power_sites
                .values()
                .map(|s| s.cost_total)
                .sum::<f64>(),
    );
    result.set(
        "cost_paid",
        store.water_sites.values().map(|s| s.cost_paid).sum::<f64>()
            + store.power_sites.values().map(|s| s.cost_paid).sum::<f64>(),
    );
    result.set("payment_due", store.total_payment_due());
    result.set("water_sites", store.water_sites.len() as i64);
    result.set("power_sites", store.power_sites.len() as i64);
    result.set("water_supply", f64::from(report.water_supply));
    result.set("power_supply", f64::from(report.power_supply));
    result.set("water_demand", report.water_demand);
    result.set("power_demand", report.power_demand);
    result.set("water_served", report.water_served);
    result.set("power_served", report.power_served);
    result.set("water_capacity", report.water_capacity);
    result.set("power_capacity", report.power_capacity);
    result.set("water_coverage", f64::from(report.water_coverage));
    result.set("power_coverage", f64::from(report.power_coverage));
    result.set(
        "water_covered_buildings",
        report.water_covered_buildings as i64,
    );
    result.set(
        "power_covered_buildings",
        report.power_covered_buildings as i64,
    );
    result.set("consumers", report.consumers as i64);
    let mut sites: Array<VarDictionary> = Array::new();
    for (kind, sites_for_kind) in [
        (UtilityKind::Water, &store.water_sites),
        (UtilityKind::Power, &store.power_sites),
    ] {
        for (&id, site) in sites_for_kind {
            let mut item = VarDictionary::new();
            item.set("placement_id", id as i64);
            item.set("kind", &GString::from(kind.as_str()));
            item.set("radius_m", f64::from(site.radius_m));
            item.set("capacity", site.capacity);
            item.set("build_days_remaining", f64::from(site.build_days_remaining));
            item.set("center_x", f64::from(site.center[0]));
            item.set("center_y", f64::from(site.center[1]));
            item.set("center_z", f64::from(site.center[2]));
            sites.push(&item);
        }
    }
    result.set("sites", &sites.to_variant());
    result.set(
        "checksum",
        &GString::from(&format!("{:016x}", store.checksum())),
    );
    result
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

fn attach_named_citizens(json: &str, save: &NamedSave) -> Result<String, String> {
    let mut value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| error.to_string())?;
    value["named_citizens"] = serde_json::to_value(save).map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
}

fn named_from_world_json(
    json: &str,
    expected_seed: u64,
) -> Result<Option<NamedCitizens>, (&'static str, String)> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| ("invalid_json", error.to_string()))?;
    let Some(raw) = value.get("named_citizens") else {
        return Ok(None);
    };
    let save: NamedSave = serde_json::from_value(raw.clone())
        .map_err(|error| ("invalid_named_citizens", error.to_string()))?;
    if save.seed != expected_seed {
        return Err((
            "seed_mismatch",
            format!(
                "named citizens use seed {} but this world is {expected_seed}",
                save.seed
            ),
        ));
    }
    let pool = NamedCitizens::from_save(save)
        .map_err(|error| ("invalid_named_citizens", error.to_string()))?;
    Ok(Some(pool))
}

fn named_pool_for_save(
    named: Option<NamedCitizens>,
    population: Option<&CityRuntime>,
    seed: u64,
) -> Result<Option<NamedCitizens>, (&'static str, String)> {
    let Some(mut pool) = named.or_else(|| population.map(|_| NamedCitizens::new(seed))) else {
        return Ok(None);
    };
    if pool.seed != seed {
        return Err((
            "seed_mismatch",
            format!(
                "named citizens use seed {} but this world is {seed}",
                pool.seed
            ),
        ));
    }
    let empty = BTreeMap::new();
    pool.sync(population.map_or(&empty, |runtime| &runtime.households));
    pool.validate()
        .map_err(|error| ("invalid_named_citizens", error.to_string()))?;
    Ok(Some(pool))
}

struct NamedLifecycleOutcome {
    tick: LifecycleTick,
    applied_deaths: u32,
    population: u64,
    count: usize,
    average_age_days: u32,
    oldest_age_days: u32,
    checksum: u64,
    death_events: Vec<CitizenDeath>,
}

fn tick_named_lifecycle(
    pool: &mut NamedCitizens,
    runtime: &mut CityRuntime,
    days: f64,
) -> NamedLifecycleOutcome {
    let outcome = pool.tick_lifecycle_with_deaths(days);
    let applied_deaths = runtime.apply_deaths(&outcome.deaths);
    // Snapshot the events before the household sync: sync may rebuild a
    // surviving slot with a different identity, and the death record must keep
    // the retired citizen's name and age.
    let death_events = outcome.death_events;
    pool.sync(&runtime.households);
    NamedLifecycleOutcome {
        tick: outcome.tick,
        applied_deaths,
        population: runtime.population(),
        count: pool.len(),
        average_age_days: pool.average_age_days(),
        oldest_age_days: pool.oldest_age_days().unwrap_or(0),
        checksum: pool.checksum(),
        death_events,
    }
}

struct MaterializationPayload {
    seed: u64,
    center: [f32; 2],
    active: Vec<MaterializedCitizen>,
    detailed: Vec<MaterializedCitizen>,
}

fn materialization_payload(
    seed: u64,
    named: Option<&NamedCitizens>,
    households: &BTreeMap<u64, Household>,
    placement_centers: &BTreeMap<u64, [f32; 2]>,
    center: [f32; 2],
    active_radius_m: f32,
    detailed_radius_m: f32,
) -> Result<MaterializationPayload, core_city::NamedError> {
    let Some(pool) = named else {
        return Ok(MaterializationPayload {
            seed,
            center,
            active: Vec::new(),
            detailed: Vec::new(),
        });
    };
    if pool.seed != seed {
        return Err(core_city::NamedError::Invalid(
            "named pool belongs to a different seed",
        ));
    }
    let mut pool = pool.clone();
    pool.sync(households);
    let selection = pool.select_near(
        center,
        placement_centers,
        MaterializationBudget::default(),
        active_radius_m,
        detailed_radius_m,
    )?;
    Ok(MaterializationPayload {
        seed,
        center,
        active: selection.active,
        detailed: selection.detailed,
    })
}

fn materialized_citizen_to_dictionary(citizen: &MaterializedCitizen) -> VarDictionary {
    let mut item = VarDictionary::new();
    item.set("id", citizen.id as i64);
    item.set("name", &GString::from(citizen.name.as_str()));
    item.set("placement_id", citizen.placement_id as i64);
    item.set("slot", i64::from(citizen.slot));
    item.set("distance_m", f64::from(citizen.distance));
    item.set("home", &PackedFloat32Array::from(citizen.home));
    item
}

fn citizen_death_to_dictionary(event: &CitizenDeath) -> VarDictionary {
    let mut item = VarDictionary::new();
    item.set("id", event.id as i64);
    item.set("placement_id", event.placement_id as i64);
    item.set("slot", i64::from(event.slot));
    item.set("name", &GString::from(event.name.as_str()));
    item.set("age_days", i64::from(event.age_days));
    item
}

fn materialization_payload_to_dictionary(payload: &MaterializationPayload) -> VarDictionary {
    let mut result = VarDictionary::new();
    result.set("ok", true);
    result.set("seed", payload.seed as i64);
    result.set("center", &PackedFloat32Array::from(payload.center));
    let mut active: Array<VarDictionary> = Array::new();
    for citizen in &payload.active {
        active.push(&materialized_citizen_to_dictionary(citizen));
    }
    let mut detailed: Array<VarDictionary> = Array::new();
    for citizen in &payload.detailed {
        detailed.push(&materialized_citizen_to_dictionary(citizen));
    }
    let mut counts = VarDictionary::new();
    counts.set("active", payload.active.len() as i64);
    counts.set("detailed", payload.detailed.len() as i64);
    result.set("counts", &counts);
    result.set("active", &active.to_variant());
    result.set("detailed", &detailed.to_variant());
    result
}

/// Largest presentation time accepted by `citizen_movement`, in seconds.
///
/// One million seconds is about 11.6 days. The bound is far inside f64's
/// exactly representable integer range, so the periodic `rem_euclid` stays
/// well-conditioned, and it rejects nonsensical clock jumps.
const MAX_MOVEMENT_TIME_S: f64 = 1_000_000.0;

/// Validated arguments for one `citizen_movement` call.
#[derive(Debug)]
struct MovementRequest {
    seed: u64,
    center: [f32; 2],
    active_radius_m: f32,
    detailed_radius_m: f32,
    time_s: f64,
}

/// Validate the raw Godot arguments before any state is locked.
fn movement_request(
    seed: i64,
    center_x: f64,
    center_z: f64,
    active_radius_m: f64,
    detailed_radius_m: f64,
    time_s: f64,
) -> Result<MovementRequest, (&'static str, String)> {
    let seed = u64::try_from(seed).map_err(|_| {
        (
            "invalid_seed",
            "world seed must be non-negative".to_string(),
        )
    })?;
    let center = [center_x as f32, center_z as f32];
    if !center.iter().all(|value| value.is_finite()) {
        return Err((
            "invalid_center",
            "movement center must be finite".to_string(),
        ));
    }
    let max_radius_m = f64::from(MAX_WORLD_COORD_M);
    for (label, value) in [("active", active_radius_m), ("detailed", detailed_radius_m)] {
        if !value.is_finite() || !(0.0..=max_radius_m).contains(&value) {
            return Err((
                "invalid_radius",
                format!("{label} movement radius must be finite and within 0..=1,000,000 m"),
            ));
        }
    }
    if !time_s.is_finite() || !(0.0..=MAX_MOVEMENT_TIME_S).contains(&time_s) {
        return Err((
            "invalid_time",
            "movement time must be finite and within 0..=1,000,000 s".to_string(),
        ));
    }
    Ok(MovementRequest {
        seed,
        center,
        active_radius_m: active_radius_m as f32,
        detailed_radius_m: detailed_radius_m as f32,
        time_s,
    })
}

/// Reject a world or household store that belongs to another seed.
fn movement_seed_error(
    seed: u64,
    world_seed: Option<u64>,
    population_seed: Option<u64>,
) -> Option<(&'static str, &'static str)> {
    if world_seed.is_some_and(|value| value != seed) {
        return Some((
            "seed_mismatch",
            "the placement store belongs to a different seed",
        ));
    }
    if population_seed.is_some_and(|value| value != seed) {
        return Some((
            "seed_mismatch",
            "the household runtime belongs to a different seed",
        ));
    }
    None
}

/// Build the traffic graph from the live road store.
///
/// A missing store is `no_roads`; a store for another seed is
/// `seed_mismatch`; an invalid segment list is `traffic_failed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MovementCacheKey {
    seed: u64,
    world_checksum: u64,
    construction_checksum: u64,
    population_checksum: u64,
    workplace_checksum: u64,
    named_checksum: u64,
    roads_checksum: u64,
    center: [u32; 2],
    active_radius: u32,
    detailed_radius: u32,
}

#[derive(Debug)]
struct MovementCache {
    key: MovementCacheKey,
    motion: CitizenMotion,
}

/// Bridge-owned movement cache entry plus cumulative lookup counters.
///
/// Counters live for the lifetime of the bridge instance. Like
/// `core_city::RouteCache`, [`MovementCacheSlot::clear`] drops the cached entry
/// but keeps `hits`, `misses`, and `builds`; it only increments `clears` so the
/// reset boundary stays visible.
#[derive(Debug, Default)]
struct MovementCacheSlot {
    entry: Option<MovementCache>,
    counters: MovementCacheCounters,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct MovementCacheCounters {
    hits: u64,
    misses: u64,
    builds: u64,
    clears: u64,
}

/// Read-only snapshot of one movement cache slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MovementCacheStats {
    open: bool,
    seed: u64,
    routes: usize,
    citizens: usize,
    hits: u64,
    misses: u64,
    builds: u64,
    clears: u64,
    checksum: u64,
}

impl MovementCacheSlot {
    /// Return the cached entry for `key`, counting a hit or a miss.
    ///
    /// A hit means the key matches and only [`CitizenMotion::positions_at`]
    /// needs to rerun for the requested time. A miss counts even when the
    /// subsequent build fails, mirroring [`RouteCache`](core_city::RouteCache).
    fn lookup(&mut self, key: &MovementCacheKey) -> Option<&MovementCache> {
        if self.entry.as_ref().is_some_and(|entry| entry.key == *key) {
            self.counters.hits += 1;
            self.entry.as_ref()
        } else {
            self.counters.misses += 1;
            None
        }
    }

    /// Store a freshly built motion and count the build.
    fn store(&mut self, key: MovementCacheKey, motion: CitizenMotion) {
        self.counters.builds += 1;
        self.entry = Some(MovementCache { key, motion });
    }

    /// Drop the entry, count the clear, and keep cumulative counters.
    fn clear(&mut self) {
        self.counters.clears += 1;
        self.entry = None;
    }

    /// Snapshot for `movement_cache_state`.
    fn stats(&self) -> MovementCacheStats {
        let counters = self.counters;
        match self.entry.as_ref() {
            Some(cache) => MovementCacheStats {
                open: true,
                seed: cache.key.seed,
                routes: cache.motion.route_count(),
                citizens: cache.motion.citizen_count(),
                hits: counters.hits,
                misses: counters.misses,
                builds: counters.builds,
                clears: counters.clears,
                checksum: cache.motion.checksum(),
            },
            None => MovementCacheStats {
                open: false,
                seed: 0,
                routes: 0,
                citizens: 0,
                hits: counters.hits,
                misses: counters.misses,
                builds: counters.builds,
                clears: counters.clears,
                checksum: 0,
            },
        }
    }
}

/// Selected movement positions for one materialization selection.
#[derive(Debug, Clone, PartialEq)]
struct MovementPayload {
    seed: u64,
    center: [f32; 2],
    time_s: f64,
    routes: usize,
    citizens: usize,
    positions: Vec<CitizenPosition>,
    /// One polyline per household route, ordered by placement id.
    route_polylines: Vec<RoutePolylinePayload>,
    checksum: u64,
}

/// One household route polyline in a movement payload.
///
/// This is display data copied from the read-only core view; the checksum stays
/// the motion checksum, so it covers exactly these routes and citizen tracks.
#[derive(Debug, Clone, PartialEq)]
struct RoutePolylinePayload {
    placement_id: u64,
    workplace_id: u64,
    points: Vec<[f32; 2]>,
    length_m: f32,
    cycle_s: f64,
}

/// Build the movement layer for the same inputs as [`materialization_payload`].
///
/// The selection is built first, then [`CitizenMotion::new`] attaches one
/// cached route per household and samples every routed citizen at `time_s`.
/// Citizens in households with no reachable workplace are omitted. Named-pool
/// or argument errors are returned unchanged from the selection layer.
#[allow(clippy::too_many_arguments)]
fn movement_motion(
    seed: u64,
    named: Option<&NamedCitizens>,
    households: &BTreeMap<u64, Household>,
    placement_centers: &BTreeMap<u64, [f32; 2]>,
    workplaces: &[Workplace],
    graph: &RoadGraph,
    center: [f32; 2],
    active_radius_m: f32,
    detailed_radius_m: f32,
) -> Result<CitizenMotion, core_city::NamedError> {
    let selection = materialization_payload(
        seed,
        named,
        households,
        placement_centers,
        center,
        active_radius_m,
        detailed_radius_m,
    )?;
    Ok(CitizenMotion::new_with_households(
        &MaterializationSelection {
            active: selection.active,
            detailed: selection.detailed,
        },
        households,
        workplaces,
        graph,
        DEFAULT_ROUTE_CACHE_CAPACITY,
    ))
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn movement_payload(
    seed: u64,
    named: Option<&NamedCitizens>,
    households: &BTreeMap<u64, Household>,
    placement_centers: &BTreeMap<u64, [f32; 2]>,
    workplaces: &[Workplace],
    graph: &RoadGraph,
    center: [f32; 2],
    active_radius_m: f32,
    detailed_radius_m: f32,
    time_s: f64,
) -> Result<MovementPayload, core_city::NamedError> {
    let motion = movement_motion(
        seed,
        named,
        households,
        placement_centers,
        workplaces,
        graph,
        center,
        active_radius_m,
        detailed_radius_m,
    )?;
    Ok(movement_payload_from_motion(seed, center, time_s, &motion))
}
const fn motion_leg_name(leg: MotionLeg) -> &'static str {
    match leg {
        MotionLeg::Outbound => "outbound",
        MotionLeg::Return => "return",
    }
}

fn movement_payload_from_motion(
    seed: u64,
    center: [f32; 2],
    time_s: f64,
    motion: &CitizenMotion,
) -> MovementPayload {
    MovementPayload {
        seed,
        center,
        time_s,
        routes: motion.route_count(),
        citizens: motion.citizen_count(),
        positions: motion.positions_at(time_s),
        route_polylines: motion
            .routes()
            .map(|route| RoutePolylinePayload {
                placement_id: route.placement_id,
                workplace_id: route.workplace_id,
                points: route.points.to_vec(),
                length_m: route.length_m,
                cycle_s: route.cycle_s,
            })
            .collect(),
        checksum: motion.checksum(),
    }
}

fn movement_payload_to_dictionary(payload: &MovementPayload) -> VarDictionary {
    let mut result = VarDictionary::new();
    result.set("ok", true);
    result.set("seed", payload.seed as i64);
    result.set("center", &PackedFloat32Array::from(payload.center));
    result.set("time_s", payload.time_s);
    result.set("routes", payload.routes as i64);
    result.set("citizens", payload.citizens as i64);
    let mut positions: Array<VarDictionary> = Array::new();
    for sample in &payload.positions {
        let mut item = VarDictionary::new();
        item.set("id", sample.id as i64);
        item.set("placement_id", sample.placement_id as i64);
        item.set("slot", i64::from(sample.slot));
        item.set("position", &PackedFloat32Array::from(sample.position));
        item.set("leg", &GString::from(motion_leg_name(sample.leg)));
        item.set("route_progress", f64::from(sample.route_progress));
        positions.push(&item);
    }
    result.set("positions", &positions.to_variant());
    let mut route_polylines: Array<VarDictionary> = Array::new();
    for route in &payload.route_polylines {
        let mut item = VarDictionary::new();
        item.set("placement_id", route.placement_id as i64);
        item.set("workplace_id", route.workplace_id as i64);
        let points: Vec<Vector2> = route
            .points
            .iter()
            .map(|point| Vector2::new(point[0], point[1]))
            .collect();
        item.set("points", &PackedVector2Array::from(points));
        item.set("length_m", f64::from(route.length_m));
        item.set("cycle_s", route.cycle_s);
        route_polylines.push(&item);
    }
    result.set("route_polylines", &route_polylines.to_variant());
    result.set(
        "checksum",
        &GString::from(&format!("{:016x}", payload.checksum)),
    );
    result
}

/// One district heatmap payload, copied from the read-only core grid.
#[derive(Debug, Clone, PartialEq)]
struct DistrictHeatmapPayload {
    seed: u64,
    heatmap: DistrictHeatmap,
}

fn district_heatmap_payload(seed: u64, heatmap: &DistrictHeatmap) -> DistrictHeatmapPayload {
    DistrictHeatmapPayload {
        seed,
        heatmap: heatmap.clone(),
    }
}

fn district_heatmap_payload_to_dictionary(payload: &DistrictHeatmapPayload) -> VarDictionary {
    let heatmap = &payload.heatmap;
    let mut result = VarDictionary::new();
    result.set("ok", true);
    result.set("seed", payload.seed as i64);
    result.set("cell_m", f64::from(heatmap.cell_m));
    result.set("cols", heatmap.cols as i64);
    result.set("rows", heatmap.rows as i64);
    result.set("center", &PackedFloat32Array::from(heatmap.center));
    result.set("radius_m", f64::from(heatmap.radius_m));
    let mut cells: Array<VarDictionary> = Array::new();
    for cell in &heatmap.cells {
        let mut item = VarDictionary::new();
        item.set("x", f64::from(cell.x));
        item.set("z", f64::from(cell.z));
        item.set("population", cell.population as i64);
        item.set("housing_capacity", cell.housing_capacity as i64);
        item.set("jobs", cell.jobs as i64);
        item.set("employed", cell.employed as i64);
        item.set("happiness", f64::from(cell.happiness));
        item.set("food", f64::from(cell.food));
        item.set("water", f64::from(cell.water));
        item.set("rest", f64::from(cell.rest));
        cells.push(&item);
    }
    result.set("cells", &cells.to_variant());
    result.set(
        "checksum",
        &GString::from(&format!("{:016x}", heatmap.checksum)),
    );
    result
}

/// One district policy payload, copied from the core registry.
#[derive(Debug, Clone, PartialEq)]
struct DistrictPolicyPayload {
    seed: u64,
    policy: DistrictPolicy,
}

fn district_policy_payload(seed: u64, policy: &DistrictPolicy) -> DistrictPolicyPayload {
    DistrictPolicyPayload {
        seed,
        policy: policy.clone(),
    }
}

fn district_policy_result(seed: u64, policy: &DistrictPolicy) -> VarDictionary {
    district_policy_payload_to_dictionary(&district_policy_payload(seed, policy))
}

fn district_policy_payload_to_dictionary(payload: &DistrictPolicyPayload) -> VarDictionary {
    let mut result = VarDictionary::new();
    result.set("ok", true);
    result.set("seed", payload.seed as i64);
    result.set(
        "policy",
        &district_policy_to_dictionary(&payload.policy).to_variant(),
    );
    result
}

fn district_policy_to_dictionary(policy: &DistrictPolicy) -> VarDictionary {
    let mut result = VarDictionary::new();
    result.set("id", policy.id as i64);
    result.set("name", &GString::from(policy.name.as_str()));
    result.set("center", &PackedFloat32Array::from(policy.center));
    result.set("radius_m", f64::from(policy.radius_m));
    result.set("tax_multiplier", f64::from(policy.tax_multiplier));
    result.set("service_funding", f64::from(policy.service_funding));
    result.set("construction_allowed", policy.construction_allowed);
    let allowlist: Vec<GString> = policy
        .blueprint_allowlist
        .iter()
        .map(|entry| GString::from(entry.as_str()))
        .collect();
    result.set(
        "blueprint_allowlist",
        &PackedStringArray::from(allowlist).to_variant(),
    );
    result
}

fn district_policy_error_result(error: &DistrictPolicyError) -> VarDictionary {
    placement_simple_error(error.code(), error.to_string())
}

/// One copy of the full district policy registry.
#[derive(Debug, Clone, PartialEq)]
struct DistrictPoliciesPayload {
    seed: u64,
    count: usize,
    policies: Vec<DistrictPolicy>,
    checksum: u64,
}

fn district_policies_payload(store: &DistrictPolicyStore) -> DistrictPoliciesPayload {
    DistrictPoliciesPayload {
        seed: store.seed,
        count: store.count(),
        policies: store.list().cloned().collect(),
        checksum: store.checksum(),
    }
}

fn district_policies_payload_to_dictionary(payload: &DistrictPoliciesPayload) -> VarDictionary {
    let mut result = VarDictionary::new();
    result.set("ok", true);
    result.set("seed", payload.seed as i64);
    result.set("count", payload.count as i64);
    let mut policies: Array<VarDictionary> = Array::new();
    for policy in &payload.policies {
        policies.push(&district_policy_to_dictionary(policy));
    }
    result.set("policies", &policies.to_variant());
    result.set(
        "checksum",
        &GString::from(&format!("{:016x}", payload.checksum)),
    );
    result
}

/// Create one policy through the bridge-owned optional registry.
///
/// The registry is created lazily for `seed`. A rejected first create removes
/// it again, so a bad request leaves no empty registry behind. A registry from
/// another seed is rejected before it is touched.
fn district_policy_create_in(
    store: &mut Option<DistrictPolicyStore>,
    seed: u64,
    request: &DistrictPolicyRequest,
) -> Result<DistrictPolicy, DistrictPolicyError> {
    if let Some(existing) = store.as_ref() {
        if existing.seed != seed {
            return Err(DistrictPolicyError::SeedMismatch {
                expected: seed,
                found: existing.seed,
            });
        }
    }
    let created = store.is_none();
    let result = store
        .get_or_insert_with(|| DistrictPolicyStore::new(seed))
        .create(request);
    if result.is_err() && created {
        *store = None;
    }
    result
}

/// Update one policy in an already-open registry.
///
/// A missing registry is reported as `not_found` for the requested id so the
/// bridge keeps one error code for "no registry" and "no such policy".
fn district_policy_update_in(
    store: Option<&mut DistrictPolicyStore>,
    seed: u64,
    id: u64,
    request: &DistrictPolicyRequest,
) -> Result<DistrictPolicy, DistrictPolicyError> {
    let Some(store) = store else {
        return Err(DistrictPolicyError::NotFound { id });
    };
    if store.seed != seed {
        return Err(DistrictPolicyError::SeedMismatch {
            expected: seed,
            found: store.seed,
        });
    }
    store.update(id, request)
}

/// Remove one policy and return the remaining count and registry checksum.
fn district_policy_remove_in(
    store: Option<&mut DistrictPolicyStore>,
    seed: u64,
    id: u64,
) -> Result<(usize, u64), DistrictPolicyError> {
    let Some(store) = store else {
        return Err(DistrictPolicyError::NotFound { id });
    };
    if store.seed != seed {
        return Err(DistrictPolicyError::SeedMismatch {
            expected: seed,
            found: store.seed,
        });
    }
    if !store.remove(id) {
        return Err(DistrictPolicyError::NotFound { id });
    }
    Ok((store.count(), store.checksum()))
}

/// Copy the registry for a state call. `None` means no registry exists yet.
fn district_policies_state_payload(
    store: Option<&DistrictPolicyStore>,
    seed: u64,
) -> Result<Option<DistrictPoliciesPayload>, DistrictPolicyError> {
    match store {
        None => Ok(None),
        Some(store) if store.seed == seed => Ok(Some(district_policies_payload(store))),
        Some(store) => Err(DistrictPolicyError::SeedMismatch {
            expected: seed,
            found: store.seed,
        }),
    }
}

#[allow(clippy::too_many_arguments)]
fn district_policy_request(
    name: GString,
    center_x: f64,
    center_z: f64,
    radius_m: f64,
    tax_multiplier: f64,
    service_funding: f64,
    construction_allowed: bool,
    allowlist: &PackedStringArray,
) -> DistrictPolicyRequest {
    DistrictPolicyRequest {
        name: name.to_string(),
        center: [center_x as f32, center_z as f32],
        radius_m: radius_m as f32,
        tax_multiplier: tax_multiplier as f32,
        service_funding: service_funding as f32,
        construction_allowed,
        blueprint_allowlist: allowlist
            .as_slice()
            .iter()
            .map(ToString::to_string)
            .collect(),
    }
}

struct RoadGraphCache {
    seed: u64,
    roads_checksum: u64,
    graph: Arc<RoadGraph>,
}

/// Reuse the derived graph while the seed and road-store checksum match.
fn cached_road_graph(
    cache: &mut Option<RoadGraphCache>,
    seed: u64,
    store: Option<&RoadStore>,
) -> Result<Arc<RoadGraph>, (&'static str, String)> {
    let Some(store) = store else {
        return Err(("no_roads", "no road store is open".to_string()));
    };
    if store.seed != seed {
        return Err((
            "seed_mismatch",
            "the road store belongs to a different seed".to_string(),
        ));
    }
    let roads_checksum = store.checksum();
    if let Some(cached) = cache
        .as_ref()
        .filter(|cached| cached.seed == seed && cached.roads_checksum == roads_checksum)
    {
        return Ok(Arc::clone(&cached.graph));
    }
    let graph = Arc::new(
        RoadGraph::build(&store.roads).map_err(|error| ("traffic_failed", error.to_string()))?,
    );
    *cache = Some(RoadGraphCache {
        seed,
        roads_checksum,
        graph: Arc::clone(&graph),
    });
    Ok(graph)
}

struct RoutePreviewState {
    seed: u64,
    graph_checksum: u64,
    cache: RouteCache,
}

struct RoutePreviewData {
    points: Vec<[f32; 2]>,
    length_m: f32,
    time_s: f32,
    cached: bool,
    hits: u64,
    misses: u64,
    checksum: u64,
}

fn route_preview_data(
    state: &mut RoutePreviewState,
    graph: &RoadGraph,
    from: [f32; 2],
    to: [f32; 2],
) -> Result<RoutePreviewData, &'static str> {
    let cached = state.cache.get(from, to).is_some();
    let route = state
        .cache
        .get_or_build(graph, from, to)
        .ok_or("route_unavailable")?;
    Ok(RoutePreviewData {
        points: route.points().to_vec(),
        length_m: route.length_m(),
        time_s: route.time_s(),
        cached,
        hits: state.cache.hits(),
        misses: state.cache.misses(),
        checksum: state.cache.checksum(),
    })
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

fn variant_i64(dictionary: &VarDictionary, key: &str) -> i64 {
    dictionary.get(key).map_or(0, |value| value.to::<i64>())
}

fn variant_u64(dictionary: &VarDictionary, key: &str) -> u64 {
    u64::try_from(variant_i64(dictionary, key)).unwrap_or(0)
}

fn variant_usize(dictionary: &VarDictionary, key: &str) -> usize {
    usize::try_from(variant_i64(dictionary, key)).unwrap_or(0)
}

fn variant_f64(dictionary: &VarDictionary, key: &str) -> f64 {
    dictionary.get(key).map_or(0.0, |value| value.to::<f64>())
}

fn variant_f32(dictionary: &VarDictionary, key: &str) -> f32 {
    variant_f64(dictionary, key) as f32
}

fn variant_bool(dictionary: &VarDictionary, key: &str) -> bool {
    dictionary.get(key).is_some_and(|value| value.to::<bool>())
}

fn advisor_payload(seed: u64, report: &AdvisorReport) -> VarDictionary {
    let mut result = VarDictionary::new();
    result.set("ok", true);
    result.set("seed", seed as i64);
    result.set("critical", report.critical() as i64);
    result.set("warnings", report.warnings() as i64);
    result.set("infos", report.infos() as i64);
    result.set("healthy", report.healthy());
    let mut messages: Array<VarDictionary> = Array::new();
    for message in &report.messages {
        let mut item = VarDictionary::new();
        item.set("code", &GString::from(&message.code));
        item.set("severity", &GString::from(message.severity.as_str()));
        item.set("title", &GString::from(&message.title));
        item.set("detail", &GString::from(&message.detail));
        let mut params = VarDictionary::new();
        for (key, value) in &message.params {
            params.set(key.as_str(), &GString::from(value.as_str()));
        }
        item.set("params", &params);
        messages.push(&item);
    }
    result.set("messages", &messages.to_variant());
    result
}

fn attach_research(json: &str, store: &ResearchStore) -> Result<String, String> {
    let mut value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| error.to_string())?;
    value["research"] = serde_json::to_value(store).map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
}

fn research_from_world_json(json: &str) -> Result<Option<ResearchStore>, (&'static str, String)> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| ("invalid_json", error.to_string()))?;
    let Some(raw) = value.get("research") else {
        return Ok(None);
    };
    let store: ResearchStore = serde_json::from_value(raw.clone())
        .map_err(|error| ("invalid_research", error.to_string()))?;
    store
        .validate(default_tech_tree())
        .map_err(|error| ("invalid_research", error.to_string()))?;
    Ok(Some(store))
}

/// Add the optional `policies` object to one already-parsed world save.
fn attach_policies(
    mut value: serde_json::Value,
    store: &DistrictPolicyStore,
) -> Result<serde_json::Value, serde_json::Error> {
    value["policies"] = serde_json::to_value(store)?;
    Ok(value)
}

/// Compose the district-policy store into the world save when one is open.
///
/// A missing store returns the input string untouched, so a world without
/// district policies keeps byte-identical JSON and checksums. A store for
/// another seed is rejected before serialization, and an invalid store is
/// rejected rather than written.
fn attach_optional_policies(
    json: String,
    policies: Option<&DistrictPolicyStore>,
    seed: u64,
) -> Result<String, (&'static str, String)> {
    let Some(policies) = policies else {
        return Ok(json);
    };
    if policies.seed != seed {
        return Err((
            "seed_mismatch",
            format!(
                "district policies use seed {} but this world is {seed}",
                policies.seed
            ),
        ));
    }
    policies
        .validate()
        .map_err(|error| ("invalid_policies", error.to_string()))?;
    let value: serde_json::Value = serde_json::from_str(&json)
        .map_err(|error| ("save_failed", format!("invalid save JSON: {error}")))?;
    let value = attach_policies(value, policies)
        .map_err(|error| ("save_failed", error.to_string()))?;
    serde_json::to_string_pretty(&value).map_err(|error| ("save_failed", error.to_string()))
}

/// Read the optional `policies` object from a world save.
///
/// Old saves have no object and load with the registry closed. A malformed
/// object, an invalid registry, or a registry for another world seed is
/// rejected so a failed load never installs a partial store.
fn policies_from_world_json(
    json: &str,
    expected_seed: u64,
) -> Result<Option<DistrictPolicyStore>, (&'static str, String)> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| ("invalid_json", error.to_string()))?;
    let Some(raw) = value.get("policies") else {
        return Ok(None);
    };
    let store: DistrictPolicyStore = serde_json::from_value(raw.clone())
        .map_err(|error| ("invalid_policies", error.to_string()))?;
    store
        .validate()
        .map_err(|error| ("invalid_policies", error.to_string()))?;
    if store.seed != expected_seed {
        return Err((
            "seed_mismatch",
            format!(
                "district policies use seed {} but this world is {expected_seed}",
                store.seed
            ),
        ));
    }
    Ok(Some(store))
}

/// Count and checksum reported by save/load when no registry is open.
fn policy_summary(policies: Option<&DistrictPolicyStore>) -> (usize, u64) {
    policies.map_or((0, 0), |store| (store.count(), store.checksum()))
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

fn starter_package_path(building: &core_city::StarterBuilding) -> Result<PathBuf, String> {
    let safe = building.id.replace(':', "_");
    let dir = std::env::temp_dir().join("townsmith_starter");
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir.join(format!("v1_{safe}.tsbp")))
}

fn build_starter_blueprint(building: &core_city::StarterBuilding) -> Result<Blueprint, String> {
    let chunks_x = (building.footprint_m[0] / 3.2).ceil().max(1.0) as u32;
    let chunks_z = (building.footprint_m[1] / 3.2).ceil().max(1.0) as u32;
    let config = VolumeConfig::new(chunks_x, building.stories.max(1) as u32, chunks_z)
        .map_err(|error| error.to_string())?;
    let mut volume = VoxelVolume::new(config).map_err(|error| error.to_string())?;
    let [width, height, depth] = config.voxels();
    let (width, height, depth) = (width as u32, height as u32, depth as u32);
    let pos = VolumePos::new;
    volume
        .fill_box(pos(0, 0, 0), pos(width, 2, depth), EDITOR_FLOOR)
        .map_err(|error| error.to_string())?;
    volume
        .fill_box(pos(0, 2, 0), pos(width, height - 2, depth), EDITOR_WALL)
        .map_err(|error| error.to_string())?;
    volume
        .fill_box(pos(2, 2, 2), pos(width - 2, height - 2, depth - 2), AIR)
        .map_err(|error| error.to_string())?;
    for story in 1..building.stories.max(1) {
        let floor_y = u32::from(story) * 32;
        if floor_y + 2 < height {
            volume
                .fill_box(
                    pos(2, floor_y, 2),
                    pos(width - 2, floor_y + 2, depth - 2),
                    EDITOR_CEILING,
                )
                .map_err(|error| error.to_string())?;
        }
    }
    volume
        .fill_box(
            pos(0, height - 2, 0),
            pos(width, height, depth),
            EDITOR_CEILING,
        )
        .map_err(|error| error.to_string())?;
    let door_x = width / 2;
    volume
        .fill_box(
            pos(door_x.saturating_sub(4), 2, 0),
            pos((door_x + 4).min(width), 22, 2),
            EDITOR_DOOR,
        )
        .map_err(|error| error.to_string())?;
    for (y, z) in [(10, 0), (10, depth - 2)] {
        let x0 = width / 4;
        volume
            .fill_box(
                pos(x0, y, z),
                pos((x0 + 8).min(width), y + 10, z + 2),
                EDITOR_WINDOW,
            )
            .map_err(|error| error.to_string())?;
    }
    for (x, y, z0, z1) in [
        (0, 10, depth / 4, depth / 4 + 8),
        (width - 2, 10, depth / 4, depth / 4 + 8),
    ] {
        volume
            .fill_box(
                pos(x, y, z0),
                pos(x + 2, y + 10, z1.min(depth)),
                EDITOR_WINDOW,
            )
            .map_err(|error| error.to_string())?;
    }
    let (materials, _) = count_materials(&volume);
    let mut metadata = BlueprintMetadata::new(building.id, building.name);
    metadata.author = "Townsmith".to_string();
    metadata.tags = vec!["starter".to_string(), building.category_str().to_string()];
    metadata.materials = materials;
    metadata.stats.resident_capacity = building.resident_capacity;
    metadata.stats.job_slots = building.job_slots;
    metadata.stats.material_cost = building.material_cost;
    let height_m = config.bounds_m()[1];
    metadata.structure = building
        .system
        .map(|system| StructureSpec::new(system, building.stories, height_m, 0.0));
    Ok(Blueprint::new(metadata, volume))
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

fn attach_services(json: &str, store: &ServiceStore) -> Result<String, String> {
    let mut value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| error.to_string())?;
    value["services"] = serde_json::to_value(store).map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
}

fn services_from_world_json(json: &str) -> Result<Option<ServiceStore>, (&'static str, String)> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| ("invalid_json", error.to_string()))?;
    let Some(raw) = value.get("services") else {
        return Ok(None);
    };
    let store: ServiceStore = serde_json::from_value(raw.clone())
        .map_err(|error| ("invalid_services", error.to_string()))?;
    store
        .validate()
        .map_err(|error| ("invalid_services", error.to_string()))?;
    Ok(Some(store))
}

fn attach_utilities(json: &str, store: &UtilityStore) -> Result<String, String> {
    let mut value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| error.to_string())?;
    value["utilities"] = serde_json::to_value(store).map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
}

fn utilities_from_world_json(json: &str) -> Result<Option<UtilityStore>, (&'static str, String)> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| ("invalid_json", error.to_string()))?;
    let Some(raw) = value.get("utilities") else {
        return Ok(None);
    };
    let store: UtilityStore = serde_json::from_value(raw.clone())
        .map_err(|error| ("invalid_utilities", error.to_string()))?;
    store
        .validate()
        .map_err(|error| ("invalid_utilities", error.to_string()))?;
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

fn sibling_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn fnv1a_bytes(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn looks_like_json_object(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    text.trim_start().starts_with('{') && serde_json::from_str::<serde_json::Value>(text).is_ok()
}

/// Write `contents` through a sibling temp file and atomically replace `path`.
///
/// A structurally valid previous save is rotated to `<path>.bak` first. The
/// temp file is removed on failure, and the parent directory is synced on Unix
/// so the rename is durable.
fn write_text_atomic(path: &Path, contents: &str) -> std::io::Result<bool> {
    let temp = sibling_with_suffix(path, ".tmp");
    let backup = sibling_with_suffix(path, ".bak");
    let mut file = std::fs::File::create(&temp)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    drop(file);

    let mut backup_rotated = false;
    if let Ok(previous) = std::fs::read(path) {
        if looks_like_json_object(&previous) {
            let mut backup_file = std::fs::File::create(&backup)?;
            backup_file.write_all(&previous)?;
            backup_file.sync_all()?;
            drop(backup_file);
            backup_rotated = true;
        }
    }

    if let Err(error) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        if let Ok(directory) = std::fs::File::open(parent) {
            let _ = directory.sync_all();
        }
    }
    Ok(backup_rotated)
}

struct ReadJsonOutcome {
    json: String,
    recovered_from_backup: bool,
    backup: Option<PathBuf>,
    backup_checksum: u64,
}

fn read_json_file(path: &Path) -> Result<String, String> {
    let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_WORLD_SAVE_BYTES {
        return Err(format!(
            "world save is {} bytes; the limit is {MAX_WORLD_SAVE_BYTES}",
            metadata.len()
        ));
    }
    std::fs::read_to_string(path).map_err(|error| error.to_string())
}

fn read_json_with_backup(path: &Path) -> Result<ReadJsonOutcome, String> {
    let primary = read_json_file(path);
    if let Ok(json) = primary {
        if looks_like_json_object(json.as_bytes()) {
            return Ok(ReadJsonOutcome {
                json,
                recovered_from_backup: false,
                backup: None,
                backup_checksum: 0,
            });
        }
    }
    let backup = sibling_with_suffix(path, ".bak");
    let json = read_json_file(&backup).map_err(|error| {
        format!("save is missing or invalid and no usable backup exists: {error}")
    })?;
    if !looks_like_json_object(json.as_bytes()) {
        return Err("save and backup are both invalid JSON objects".to_string());
    }
    Ok(ReadJsonOutcome {
        backup_checksum: fnv1a_bytes(json.as_bytes()),
        recovered_from_backup: true,
        backup: Some(backup),
        json,
    })
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

/// Display color for an editor-palette material.
///
/// The editor still uses a single neutral override, but placed blueprints read
/// the vertex colors so doors, windows, roofs, and stone remain legible at
/// city distance. Unknown materials fall back to the warm timber tone.
#[must_use]
fn voxel_material_color(material: MaterialId) -> [f32; 3] {
    match material {
        EDITOR_WALL => [0.72, 0.62, 0.48],
        EDITOR_FLOOR => [0.52, 0.50, 0.46],
        EDITOR_CEILING => [0.35, 0.30, 0.27],
        EDITOR_WINDOW => [0.35, 0.62, 0.82],
        EDITOR_DOOR => [0.30, 0.18, 0.10],
        _ => [0.75, 0.70, 0.60],
    }
}

/// Upload one engine-neutral voxel mesh as a single packed-array surface.
///
/// Vertex colors carry the palette material so placed blueprints can show
/// doors and windows without one draw call per material. The editor still
/// overrides the surface with one neutral material. UVs are passed through in
/// meters.
fn voxel_mesh_to_godot(buffers: &MeshBuffers) -> Result<Gd<ArrayMesh>, &'static str> {
    if buffers.vertices.is_empty() || buffers.indices.is_empty() {
        return Ok(ArrayMesh::new_gd());
    }

    let vertex_count = buffers.vertices.len();
    let mut vertices = Vec::with_capacity(vertex_count);
    let mut normals = Vec::with_capacity(vertex_count);
    let mut uvs = Vec::with_capacity(vertex_count);
    let mut colors = Vec::with_capacity(vertex_count);

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
        let color = voxel_material_color(vertex.material);
        colors.push(Color::from_rgb(color[0], color[1], color[2]));
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
        ArrayType::COLOR.ord() as usize,
        &PackedColorArray::from(colors),
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
mod save_compat_tests;

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
    fn district_heatmap_payload_copies_grid_and_checksum() {
        let heatmap = DistrictHeatmap {
            cell_m: 16.0,
            cols: 2,
            rows: 2,
            center: [100.0, 200.0],
            radius_m: 16.0,
            cells: vec![
                core_city::DistrictCell {
                    x: 92.0,
                    z: 192.0,
                    population: 4,
                    housing_capacity: 5,
                    jobs: 3,
                    employed: 2,
                    happiness: 0.8,
                    food: 0.9,
                    water: 0.7,
                    rest: 0.6,
                },
                core_city::DistrictCell {
                    x: 108.0,
                    z: 192.0,
                    population: 0,
                    housing_capacity: 0,
                    jobs: 0,
                    employed: 0,
                    happiness: 0.0,
                    food: 0.0,
                    water: 0.0,
                    rest: 0.0,
                },
            ],
            checksum: 0x1234_5678_9abc_def0,
        };
        let payload = district_heatmap_payload(7, &heatmap);
        assert_eq!(payload.seed, 7);
        assert_eq!(payload.heatmap, heatmap);
        assert_eq!(payload.heatmap.cells.len(), 2);
        assert_eq!(payload.heatmap.checksum, 0x1234_5678_9abc_def0);
        let empty = DistrictHeatmap::default();
        let payload = district_heatmap_payload(7, &empty);
        assert!(payload.heatmap.cells.is_empty());
        assert_eq!(payload.heatmap.cols, 0);
        assert_eq!(payload.heatmap.rows, 0);
    }

    fn district_policy_request(name: &str) -> DistrictPolicyRequest {
        DistrictPolicyRequest {
            name: name.to_string(),
            center: [100.0, 100.0],
            radius_m: 64.0,
            tax_multiplier: 1.0,
            service_funding: 1.0,
            construction_allowed: true,
            blueprint_allowlist: std::collections::BTreeSet::new(),
        }
    }

    #[test]
    fn district_policy_bridge_lazily_creates_and_copies_state() {
        let mut registry: Option<DistrictPolicyStore> = None;
        assert!(district_policies_state_payload(registry.as_ref(), 2026)
            .expect("empty state")
            .is_none());

        let policy =
            district_policy_create_in(&mut registry, 2026, &district_policy_request("Riverside"))
                .expect("lazy create");
        assert_eq!(policy.id, 1);
        let store = registry.as_ref().expect("registry was created");
        assert_eq!(store.count(), 1);
        assert_eq!(store.seed, 2026);

        let state = district_policies_state_payload(registry.as_ref(), 2026)
            .expect("state")
            .expect("registry exists");
        assert_eq!(state.seed, 2026);
        assert_eq!(state.count, 1);
        assert_eq!(state.policies, vec![policy]);
        assert_eq!(state.checksum, store.checksum());
    }

    #[test]
    fn district_policy_bridge_rolls_back_a_failed_lazy_create() {
        let mut registry: Option<DistrictPolicyStore> = None;
        let mut invalid = district_policy_request("Riverside");
        invalid.radius_m = 1.0;
        assert_eq!(
            district_policy_create_in(&mut registry, 2026, &invalid),
            Err(DistrictPolicyError::InvalidRadius { radius_m: 1.0 })
        );
        assert!(registry.is_none(), "a failed first create left a registry");

        let created =
            district_policy_create_in(&mut registry, 2026, &district_policy_request("Riverside"))
                .expect("create");
        assert_eq!(
            district_policy_create_in(&mut registry, 2026, &invalid),
            Err(DistrictPolicyError::InvalidRadius { radius_m: 1.0 })
        );
        let store = registry.as_ref().expect("the open registry is kept");
        assert_eq!(store.count(), 1);
        assert_eq!(store.get(created.id), Some(&created));
    }

    #[test]
    fn district_policy_bridge_rejects_a_foreign_seed_without_touching_the_store() {
        let mut registry = Some(DistrictPolicyStore::new(2026));
        assert_eq!(
            district_policy_create_in(&mut registry, 2027, &district_policy_request("Riverside")),
            Err(DistrictPolicyError::SeedMismatch {
                expected: 2027,
                found: 2026
            })
        );
        let store = registry.as_ref().expect("registry is kept");
        assert!(store.is_empty());
        assert_eq!(store.seed, 2026);
        assert_eq!(store.next_id, 1);
    }

    #[test]
    fn district_policy_bridge_update_and_remove_use_the_open_store() {
        let mut registry = Some(DistrictPolicyStore::new(2026));
        let first =
            district_policy_create_in(&mut registry, 2026, &district_policy_request("Riverside"))
                .expect("first");
        let second =
            district_policy_create_in(&mut registry, 2026, &district_policy_request("Old Town"))
                .expect("second");

        let mut changed = district_policy_request("Harbour");
        changed.tax_multiplier = 2.0;
        changed.construction_allowed = false;
        changed
            .blueprint_allowlist
            .insert("base:workshop".to_string());
        let updated =
            district_policy_update_in(registry.as_mut(), 2026, first.id, &changed).expect("update");
        assert_eq!(updated.id, first.id);
        assert_eq!(updated.name, "Harbour");
        assert_eq!(updated.tax_multiplier, 2.0);
        assert!(!updated.construction_allowed);
        assert_eq!(
            updated.blueprint_allowlist,
            std::collections::BTreeSet::from(["base:workshop".to_string()])
        );

        let (count, checksum) =
            district_policy_remove_in(registry.as_mut(), 2026, first.id).expect("remove");
        let store = registry.as_ref().expect("registry");
        assert_eq!(count, 1);
        assert_eq!(checksum, store.checksum());
        assert!(store.get(first.id).is_none());
        assert_eq!(store.get(second.id), Some(&second));
    }

    #[test]
    fn district_policy_bridge_update_remove_and_state_reject_foreign_or_missing_stores() {
        let mut missing: Option<DistrictPolicyStore> = None;
        let request = district_policy_request("Riverside");
        assert_eq!(
            district_policy_update_in(missing.as_mut(), 2026, 1, &request),
            Err(DistrictPolicyError::NotFound { id: 1 })
        );
        assert_eq!(
            district_policy_remove_in(missing.as_mut(), 2026, 1),
            Err(DistrictPolicyError::NotFound { id: 1 })
        );

        let mut registry = Some(DistrictPolicyStore::new(2026));
        let policy = district_policy_create_in(&mut registry, 2026, &request).expect("create");
        assert_eq!(
            district_policy_update_in(registry.as_mut(), 2027, policy.id, &request),
            Err(DistrictPolicyError::SeedMismatch {
                expected: 2027,
                found: 2026
            })
        );
        assert_eq!(
            district_policy_remove_in(registry.as_mut(), 2027, policy.id),
            Err(DistrictPolicyError::SeedMismatch {
                expected: 2027,
                found: 2026
            })
        );
        assert_eq!(
            district_policies_state_payload(registry.as_ref(), 2027),
            Err(DistrictPolicyError::SeedMismatch {
                expected: 2027,
                found: 2026
            })
        );
        // A rejected foreign-seed call leaves the record and counter alone.
        let store = registry.as_ref().expect("registry");
        assert_eq!(store.get(policy.id), Some(&policy));
        assert_eq!(store.next_id, 2);
    }

    #[test]
    fn district_policy_payloads_copy_records_and_checksum() {
        let mut store = DistrictPolicyStore::new(2026);
        let mut request = district_policy_request("Riverside");
        request
            .blueprint_allowlist
            .insert("base:workshop".to_string());
        let policy = store.create(&request).expect("create");

        let payload = district_policy_payload(2026, &policy);
        assert_eq!(payload.seed, 2026);
        assert_eq!(payload.policy, policy);
        assert_eq!(payload.policy.name, "Riverside");
        assert_eq!(payload.policy.center, [100.0, 100.0]);
        assert_eq!(payload.policy.radius_m, 64.0);
        assert_eq!(
            payload.policy.blueprint_allowlist,
            std::collections::BTreeSet::from(["base:workshop".to_string()])
        );

        let state = district_policies_payload(&store);
        assert_eq!(state.seed, 2026);
        assert_eq!(state.count, 1);
        assert_eq!(state.policies, vec![policy]);
        assert_eq!(state.checksum, store.checksum());
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
    fn city_layout_rejects_negative_seed_but_accepts_zero() {
        assert_eq!(city_layout_seed(-1), Err("invalid_seed"));
        assert_eq!(city_layout_seed(0), Ok(0));
        assert_eq!(city_layout_seed(7), Ok(7));
    }

    #[test]
    fn lot_validate_rejects_a_foreign_seed_store() {
        let store = LotStore::new(2026);
        assert!(lot_store_seed_matches(Some(&store), 2026));
        assert!(!lot_store_seed_matches(Some(&store), 2027));
        assert!(lot_store_seed_matches(None, 2027));
    }

    #[test]
    fn acceptance_house_installs_the_default_volume_and_props() {
        let state = acceptance_house_editor_state();
        assert_eq!(state.volume.config().voxels(), [160, 96, 160]);
        assert!(!state.props.is_empty());
    }

    #[test]
    fn road_width_uses_the_core_range() {
        assert!(!road_width_valid(1));
        assert!(road_width_valid(2));
        assert!(road_width_valid(24));
        assert!(!road_width_valid(25));
        assert!(!road_width_valid(-1));
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
    fn construction_time_scale_parsing_validates_modes_and_rates() {
        assert_eq!(
            parse_construction_time_scale("developer", f64::NAN).expect("developer"),
            ConstructionTimeScale::DeveloperSeconds
        );
        assert_eq!(
            parse_construction_time_scale(" campaign ", 60.0).expect("campaign"),
            ConstructionTimeScale::campaign(60.0).expect("scale")
        );
        assert!(parse_construction_time_scale("campaign", 9.99).is_err());
        assert!(parse_construction_time_scale("campaign", 3600.1).is_err());
        assert!(parse_construction_time_scale("campaign", f64::NAN).is_err());
        assert!(parse_construction_time_scale("campaign", f64::INFINITY).is_err());
        for mode in [
            "",
            "Developer",
            "campaign_days",
            "developer_seconds",
            "fast",
        ] {
            assert_eq!(
                parse_construction_time_scale(mode, 60.0)
                    .expect_err("mode")
                    .0,
                "invalid_time_scale_mode"
            );
        }
        assert_eq!(
            parse_construction_time_scale("campaign", 0.0)
                .expect_err("rate")
                .0,
            "invalid_time_scale"
        );
    }

    #[test]
    fn construction_time_scale_checksum_tracks_mode_and_rate() {
        let developer = ConstructionTimeScale::DeveloperSeconds;
        let campaign = ConstructionTimeScale::campaign(60.0).expect("scale");
        let slower = ConstructionTimeScale::campaign(120.0).expect("scale");
        assert_eq!(
            construction_time_scale_checksum(developer),
            construction_time_scale_checksum(ConstructionTimeScale::default())
        );
        assert_ne!(
            construction_time_scale_checksum(developer),
            construction_time_scale_checksum(campaign)
        );
        assert_ne!(
            construction_time_scale_checksum(campaign),
            construction_time_scale_checksum(slower)
        );
    }

    #[test]
    fn campaign_time_scale_round_trips_through_world_save() {
        let mut store = ConstructionStore::new(4, 1_000_000);
        store.add_site(7, 64_224).expect("site");
        store.tick(10.0);
        store
            .set_time_scale(ConstructionTimeScale::campaign(60.0).expect("scale"))
            .expect("set scale");
        let json = attach_construction("{}", &store).expect("attach");
        let decoded = construction_from_world_json(&json)
            .expect("parse")
            .expect("construction field");
        assert_eq!(decoded.time_scale(), store.time_scale());
        assert_eq!(decoded.checksum(), store.checksum());
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
    fn research_world_json_round_trips_and_old_saves_are_closed() {
        let tree = TechTree::default_tree();
        let mut store = ResearchStore::new(2026);
        store
            .unlock("construction.timber_frame", &tree, Age::Wood)
            .expect("unlock");
        let json = attach_research("{}", &store).expect("attach");
        let decoded = research_from_world_json(&json)
            .expect("parse")
            .expect("present");
        assert_eq!(decoded, store);
        assert!(research_from_world_json("{}").expect("old save").is_none());
    }

    #[test]
    fn research_world_json_rejects_unknown_completed_nodes() {
        let mut store = ResearchStore::new(2026);
        store.completed.insert("missing.node".to_string());
        let json = attach_research("{}", &store).expect("attach");
        let error = research_from_world_json(&json).expect_err("reject");
        assert_eq!(error.0, "invalid_research");
    }

    #[test]
    fn structure_age_requirements_follow_the_city_age() {
        let brick = StructureSpec::new(StructuralSystem::LoadBearingBrick, 4, 12.0, 7.0);
        assert_eq!(
            structure_age_error(&brick, Age::Wood).expect("locked").0,
            "age_locked"
        );
        assert!(structure_age_error(&brick, Age::Masonry).is_none());
        let timber = StructureSpec::new(StructuralSystem::TimberFrame, 2, 6.0, 6.0);
        assert!(structure_age_error(&timber, Age::Wood).is_none());
        let composite = timber.with_ground_system(StructuralSystem::LoadBearingBrick);
        let error = structure_age_error(&composite, Age::Wood).expect("ground locked");
        assert!(error.1.contains("load-bearing brick"), "{}", error.1);
        assert!(structure_age_error(&composite, Age::Masonry).is_none());
    }

    #[test]
    fn economy_world_json_round_trips_and_old_saves_are_closed() {
        let mut economy = Economy::new(2026);
        economy
            .take_loan(
                40_000.0,
                core_city::DEFAULT_LOAN_RATE_PER_DAY,
                core_city::DEFAULT_LOAN_TERM_DAYS,
            )
            .expect("loan");
        economy.tick(3600.0, 50, 40, 10);
        let json = attach_economy("{}", &economy).expect("attach");
        let decoded = economy_from_world_json(&json)
            .expect("parse")
            .expect("present");
        assert!((decoded.balance - economy.balance).abs() < 0.001);
        assert!((decoded.debt() - economy.debt()).abs() < 0.001);
        assert!(economy_from_world_json("{}").expect("old save").is_none());
    }

    #[test]
    fn economy_world_json_accepts_pre_loan_saves() {
        let mut economy = Economy::new(2026);
        economy.tick(3600.0, 50, 40, 10);
        let json = attach_economy("{}", &economy).expect("attach");
        let mut value: serde_json::Value = serde_json::from_str(&json).expect("json");
        value["economy"]
            .as_object_mut()
            .expect("economy object")
            .remove("loans");
        let decoded = economy_from_world_json(&value.to_string())
            .expect("parse")
            .expect("present");
        assert!(decoded.loans.is_empty());
        assert_eq!(decoded.debt(), 0.0);
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
    fn utility_world_json_round_trips_and_old_saves_are_closed() {
        let mut store = UtilityStore::new(2026);
        store
            .add_site(1, UtilityKind::Water, [10.0, 40.0, 20.0])
            .expect("water");
        store
            .add_site(1, UtilityKind::Power, [10.0, 40.0, 20.0])
            .expect("power");
        store.refresh(&[UtilityConsumer {
            id: 1,
            center: [10.0, 20.0],
            residents: 2,
            workers: 2,
        }]);
        let json = attach_utilities("{}", &store).expect("attach");
        let decoded = utilities_from_world_json(&json)
            .expect("parse")
            .expect("present");
        assert_eq!(decoded, store);
        assert!(utilities_from_world_json("{}").expect("old save").is_none());
    }

    #[test]
    fn utility_world_json_rejects_invalid_sites() {
        let mut store = UtilityStore::new(2026);
        store
            .add_site(1, UtilityKind::Water, [0.0, 0.0, 0.0])
            .expect("water");
        store.water_sites.get_mut(&1).expect("site").radius_m = -1.0;
        let json = serde_json::to_string(&serde_json::json!({"utilities": store})).expect("json");
        let error = utilities_from_world_json(&json).expect_err("reject");
        assert_eq!(error.0, "invalid_utilities");
    }

    #[test]
    fn service_world_json_round_trips_and_old_saves_are_closed() {
        let mut store = ServiceStore::new(2026);
        store
            .add_site(
                1,
                ServiceKind::School,
                ServiceTier::Improved,
                [10.0, 40.0, 20.0],
            )
            .expect("school");
        store.refresh(&[ServiceConsumer {
            id: 1,
            center: [10.0, 20.0],
            residents: 2,
            workers: 2,
        }]);
        let json = attach_services("{}", &store).expect("attach");
        let decoded = services_from_world_json(&json)
            .expect("parse")
            .expect("present");
        assert_eq!(decoded, store);
        assert!(services_from_world_json("{}").expect("old save").is_none());
    }

    #[test]
    fn service_world_json_rejects_invalid_sites() {
        let mut store = ServiceStore::new(2026);
        store
            .add_site(1, ServiceKind::Clinic, ServiceTier::Basic, [0.0, 0.0, 0.0])
            .expect("clinic");
        store.sites.get_mut(&1).expect("site").capacity = -1.0;
        let json = serde_json::to_string(&serde_json::json!({"services": store})).expect("json");
        let error = services_from_world_json(&json).expect_err("reject");
        assert_eq!(error.0, "invalid_services");
    }

    #[test]
    fn civic_research_adds_to_the_population_rate() {
        assert_eq!(research_rate_per_day(500, 0.0), 25.0);
        assert_eq!(research_rate_per_day(500, 20.0), 45.0);
        assert_eq!(research_rate_per_day(0, 8.0), 8.0);
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
    fn editor_blueprint_preserves_composite_structure_metadata() {
        let blueprint = blueprint_from_editor_volume(
            "local:composite".to_string(),
            "Composite".to_string(),
            room_demo::acceptance_house(),
            &[],
            StructuralSystem::TimberFrame,
            Some(StructuralSystem::LoadBearingStone),
            6.0,
            Some(0.5),
        );
        let structure = blueprint.metadata.structure.expect("structure");
        assert_eq!(
            structure.ground_system,
            Some(StructuralSystem::LoadBearingStone)
        );
        assert_eq!(structure.max_span_m, 6.0);
        assert_eq!(structure.foundation_depth_m, 0.5);
        assert_eq!(structure.stories, 2);
        assert!(structure.summary().expect("valid").composite_bonus);
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
            StructuralSystem::TimberFrame,
            None,
            0.0,
            None,
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
    fn named_citizens_round_trip_in_world_json() {
        let mut runtime = CityRuntime::new(2026);
        runtime.tick(2.0, 2, &[(1, 2)]);
        let mut named = NamedCitizens::new(2026);
        named.sync(&runtime.households);
        let json = attach_named_citizens("{}", &named.to_save()).expect("attach");
        let parsed = named_from_world_json(&json, 2026)
            .expect("parse")
            .expect("present");
        assert_eq!(parsed, named);
        assert!(named_from_world_json("{}", 2026)
            .expect("old save")
            .is_none());
        assert!(named_from_world_json(&json, 2027).is_err());
    }

    #[test]
    fn materialization_payload_orders_and_caps_citizens() {
        let mut runtime = CityRuntime::new(2026);
        runtime.tick(10.0, 8, &[(1, 4), (2, 4)]);
        let mut named = NamedCitizens::new(2026);
        named.sync(&runtime.households);
        let centers = BTreeMap::from([(1, [0.0_f32, 0.0]), (2, [100.0, 0.0])]);
        let payload = materialization_payload(
            2026,
            Some(&named),
            &runtime.households,
            &centers,
            [0.0, 0.0],
            50.0,
            50.0,
        )
        .expect("payload");
        assert_eq!(payload.seed, 2026);
        assert_eq!(payload.active.len(), 4);
        assert_eq!(payload.detailed.len(), 4);
        assert!(payload
            .active
            .iter()
            .all(|citizen| citizen.placement_id == 1));
        for window in payload.active.windows(2) {
            assert!(
                window[0].distance < window[1].distance
                    || (window[0].distance == window[1].distance && window[0].id < window[1].id)
            );
        }
        assert_eq!(
            payload.active[0].name,
            core_city::name_for(payload.active[0].id)
        );
        assert_eq!(&payload.detailed[..], &payload.active[..]);
    }

    fn movement_fixture() -> (
        CityRuntime,
        NamedCitizens,
        BTreeMap<u64, [f32; 2]>,
        Vec<Workplace>,
        RoadGraph,
    ) {
        let mut runtime = CityRuntime::new(2026);
        runtime.tick(10.0, 4, &[(1, 4)]);
        let mut named = NamedCitizens::new(2026);
        named.sync(&runtime.households);
        let centers = BTreeMap::from([(1, [10.0_f32, 0.0])]);
        let workplaces = vec![Workplace {
            id: 2,
            slots: 4,
            center: [90.0, 0.0],
        }];
        let graph = RoadGraph::build(&[RoadSegment {
            id: 1,
            start: [0, 0],
            end: [100, 0],
            width_m: 6,
        }])
        .expect("graph");
        (runtime, named, centers, workplaces, graph)
    }

    #[test]
    fn movement_payload_is_deterministic_and_routed() {
        let (runtime, named, centers, workplaces, graph) = movement_fixture();
        let first = movement_payload(
            2026,
            Some(&named),
            &runtime.households,
            &centers,
            &workplaces,
            &graph,
            [10.0, 0.0],
            50.0,
            50.0,
            0.0,
        )
        .expect("payload");
        let second = movement_payload(
            2026,
            Some(&named),
            &runtime.households,
            &centers,
            &workplaces,
            &graph,
            [10.0, 0.0],
            50.0,
            50.0,
            0.0,
        )
        .expect("payload");
        assert_eq!(first, second);
        assert_eq!(first.routes, 1);
        assert_eq!(first.citizens, 4);
        assert_eq!(first.positions.len(), 4);
        assert!(first.checksum != 0);
        assert!(first
            .positions
            .iter()
            .all(|position| position.position.iter().all(|value| value.is_finite())));
        let later = movement_payload(
            2026,
            Some(&named),
            &runtime.households,
            &centers,
            &workplaces,
            &graph,
            [10.0, 0.0],
            50.0,
            50.0,
            20.0,
        )
        .expect("payload");
        assert!(later
            .positions
            .iter()
            .any(|position| position.route_progress > 0.0));
    }

    #[test]
    fn movement_payload_exposes_route_polylines_in_home_to_work_order() {
        let (runtime, named, centers, workplaces, graph) = movement_fixture();
        let payload = movement_payload(
            2026,
            Some(&named),
            &runtime.households,
            &centers,
            &workplaces,
            &graph,
            [10.0, 0.0],
            50.0,
            50.0,
            0.0,
        )
        .expect("payload");
        assert_eq!(payload.route_polylines.len(), 1);
        let route = &payload.route_polylines[0];
        assert_eq!(route.placement_id, 1);
        assert_eq!(route.workplace_id, 2);
        assert_eq!(route.points.first().copied(), Some([10.0, 0.0]));
        assert_eq!(route.points.last().copied(), Some([90.0, 0.0]));
        let expected_length: f32 = route
            .points
            .windows(2)
            .map(|pair| {
                let dx = pair[1][0] - pair[0][0];
                let dz = pair[1][1] - pair[0][1];
                (dx * dx + dz * dz).sqrt()
            })
            .sum();
        assert!((route.length_m - expected_length).abs() < 0.01);
        assert!(route.cycle_s > 0.0);
        let later = movement_payload(
            2026,
            Some(&named),
            &runtime.households,
            &centers,
            &workplaces,
            &graph,
            [10.0, 0.0],
            50.0,
            50.0,
            20.0,
        )
        .expect("later payload");
        assert_eq!(later.route_polylines, payload.route_polylines);
        assert_eq!(later.checksum, payload.checksum);
    }

    #[test]
    fn movement_payload_omits_households_without_a_workplace() {
        let (runtime, named, centers, _workplaces, graph) = movement_fixture();
        let payload = movement_payload(
            2026,
            Some(&named),
            &runtime.households,
            &centers,
            &[],
            &graph,
            [10.0, 0.0],
            50.0,
            50.0,
            0.0,
        )
        .expect("payload");
        assert!(payload.positions.is_empty());
        assert_eq!(payload.routes, 0);
    }

    #[test]
    fn road_graph_cache_decisions() {
        let mut cache = None;
        assert_eq!(
            cached_road_graph(&mut cache, 2026, None)
                .expect_err("no store")
                .0,
            "no_roads"
        );
        let mut store = RoadStore::new(2026);
        store.roads.push(RoadSegment {
            id: 1,
            start: [0, 0],
            end: [10, 0],
            width_m: 6,
        });
        let first = cached_road_graph(&mut cache, 2026, Some(&store)).expect("first");
        let second = cached_road_graph(&mut cache, 2026, Some(&store)).expect("second");
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(
            cache.as_ref().expect("cache").roads_checksum,
            store.checksum()
        );
        let before_edit = Arc::clone(&first);
        store.roads.push(RoadSegment {
            id: 2,
            start: [10, 0],
            end: [20, 0],
            width_m: 6,
        });
        let after_edit = cached_road_graph(&mut cache, 2026, Some(&store)).expect("edited");
        assert!(!Arc::ptr_eq(&before_edit, &after_edit));
        let cached_after_edit = Arc::clone(&cache.as_ref().expect("cache").graph);
        assert_eq!(
            cached_road_graph(&mut cache, 2027, Some(&store))
                .expect_err("seed mismatch")
                .0,
            "seed_mismatch"
        );
        assert!(Arc::ptr_eq(
            &cached_after_edit,
            &cache.as_ref().expect("cache unchanged").graph
        ));
        let mut empty_cache = None;
        let empty = RoadStore::new(2026);
        let empty_first = cached_road_graph(&mut empty_cache, 2026, Some(&empty)).expect("empty");
        let empty_second =
            cached_road_graph(&mut empty_cache, 2026, Some(&empty)).expect("empty reuse");
        assert!(Arc::ptr_eq(&empty_first, &empty_second));
    }

    #[test]
    fn movement_cache_key_tracks_every_input() {
        let base = MovementCacheKey {
            seed: 2026,
            world_checksum: 1,
            construction_checksum: 2,
            population_checksum: 3,
            workplace_checksum: 6,
            named_checksum: 4,
            roads_checksum: 5,
            center: [10.0_f32.to_bits(), 20.0_f32.to_bits()],
            active_radius: 50.0_f32.to_bits(),
            detailed_radius: 20.0_f32.to_bits(),
        };
        assert_eq!(base, base);
        let mut changed = base;
        changed.seed += 1;
        assert_ne!(base, changed);
        let mut changed = base;
        changed.roads_checksum += 1;
        assert_ne!(base, changed);
        let mut changed = base;
        changed.center[0] = 11.0_f32.to_bits();
        assert_ne!(base, changed);
        let mut changed = base;
        changed.active_radius = 51.0_f32.to_bits();
        assert_ne!(base, changed);
    }

    fn movement_cache_fixture() -> (MovementCacheKey, CitizenMotion) {
        let key = MovementCacheKey {
            seed: 2026,
            world_checksum: 1,
            construction_checksum: 2,
            population_checksum: 3,
            workplace_checksum: 6,
            named_checksum: 4,
            roads_checksum: 5,
            center: [0.0_f32.to_bits(), 0.0_f32.to_bits()],
            active_radius: 50.0_f32.to_bits(),
            detailed_radius: 20.0_f32.to_bits(),
        };
        let motion = CitizenMotion::new(
            &MaterializationSelection::default(),
            &[],
            &RoadGraph::build(&[]).expect("empty graph"),
            DEFAULT_ROUTE_CACHE_CAPACITY,
        );
        (key, motion)
    }

    #[test]
    fn movement_cache_counts_hits_misses_builds_and_clears() {
        let mut slot = MovementCacheSlot::default();
        let (key, motion) = movement_cache_fixture();
        assert_eq!(slot.stats().hits, 0);
        assert!(!slot.stats().open);

        assert!(slot.lookup(&key).is_none());
        slot.store(key, motion.clone());
        let stats = slot.stats();
        assert_eq!(
            (stats.hits, stats.misses, stats.builds, stats.clears),
            (0, 1, 1, 0)
        );
        assert!(stats.open);

        let hit = slot.lookup(&key).expect("hit");
        assert_eq!(hit.motion.checksum(), motion.checksum());
        let stats = slot.stats();
        assert_eq!(
            (stats.hits, stats.misses, stats.builds, stats.clears),
            (1, 1, 1, 0)
        );

        let mut changed = key;
        changed.seed += 1;
        assert!(slot.lookup(&changed).is_none());
        let stats = slot.stats();
        assert_eq!(
            (stats.hits, stats.misses, stats.builds, stats.clears),
            (1, 2, 1, 0)
        );

        slot.clear();
        let stats = slot.stats();
        assert!(!stats.open);
        assert_eq!(
            (stats.hits, stats.misses, stats.builds, stats.clears),
            (1, 2, 1, 1)
        );
        assert_eq!(stats.checksum, 0);
    }

    #[test]
    fn movement_cache_counters_are_repeatable() {
        let run = || {
            let mut slot = MovementCacheSlot::default();
            let (key, motion) = movement_cache_fixture();
            let _ = slot.lookup(&key);
            slot.store(key, motion.clone());
            let _ = slot.lookup(&key);
            slot.clear();
            slot.stats()
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn movement_request_validates_time_and_radii() {
        assert_eq!(
            movement_request(2026, 0.0, 0.0, 10.0, 5.0, f64::NAN)
                .expect_err("nan time")
                .0,
            "invalid_time"
        );
        assert_eq!(
            movement_request(2026, 0.0, 0.0, 10.0, 5.0, -1.0)
                .expect_err("negative time")
                .0,
            "invalid_time"
        );
        assert_eq!(
            movement_request(2026, 0.0, 0.0, 10.0, 5.0, 1_000_001.0)
                .expect_err("too large")
                .0,
            "invalid_time"
        );
        assert_eq!(
            movement_request(2026, 0.0, 0.0, -1.0, 5.0, 0.0)
                .expect_err("negative radius")
                .0,
            "invalid_radius"
        );
        assert_eq!(
            movement_request(2026, f64::INFINITY, 0.0, 10.0, 5.0, 0.0)
                .expect_err("bad center")
                .0,
            "invalid_center"
        );
        assert!(movement_request(2026, 0.0, 0.0, 10.0, 5.0, 0.0).is_ok());
    }

    #[test]
    fn movement_seed_error_checks_world_and_population() {
        assert!(movement_seed_error(2026, Some(2026), Some(2026)).is_none());
        assert_eq!(
            movement_seed_error(2026, Some(2027), None)
                .expect("world")
                .0,
            "seed_mismatch"
        );
        assert_eq!(
            movement_seed_error(2026, None, Some(2027))
                .expect("population")
                .0,
            "seed_mismatch"
        );
    }

    #[test]
    fn named_lifecycle_tick_ages_and_reaps_at_the_limit() {
        let mut pool = NamedCitizens::new(2026);
        for slot in 0..3_u32 {
            let id = core_city::citizen_id(2026, 1, slot);
            pool.citizens.insert(
                (1, slot),
                core_city::NamedCitizen {
                    id,
                    placement_id: 1,
                    slot,
                    name: core_city::name_for(id),
                    age_days: if slot == 0 {
                        40 * 365
                    } else {
                        core_city::MAX_AGE_DAYS - 5
                    },
                },
            );
        }
        let before = pool.checksum();
        let mut runtime = CityRuntime::new(2026);
        runtime.sync(&[(1, 3)]);
        runtime.tick(10.0, 3, &[(1, 3)]);
        let outcome = tick_named_lifecycle(&mut pool, &mut runtime, 10.0);
        assert_eq!(outcome.tick.aged, 3);
        assert_eq!(outcome.tick.died, 2);
        assert_eq!(outcome.applied_deaths, 2);
        assert_eq!(outcome.population, 1);
        assert_eq!(outcome.count, 1);
        assert_eq!(outcome.average_age_days, 40 * 365 + 10);
        assert_eq!(outcome.oldest_age_days, 40 * 365 + 10);
        assert_ne!(outcome.checksum, before);
        assert_eq!(outcome.death_events.len(), 2);
        for (event, slot) in outcome.death_events.iter().zip([1_u32, 2]) {
            let id = core_city::citizen_id(2026, 1, slot);
            assert_eq!(event.id, id);
            assert_eq!(event.placement_id, 1);
            assert_eq!(event.slot, slot);
            assert_eq!(event.name, core_city::name_for(id));
            assert_eq!(event.age_days, core_city::MAX_AGE_DAYS);
            assert!(pool.get(1, slot).is_none());
        }
    }

    #[test]
    fn named_lifecycle_tick_returns_no_events_when_nobody_dies() {
        let mut pool = NamedCitizens::new(2026);
        let id = core_city::citizen_id(2026, 1, 0);
        pool.citizens.insert(
            (1, 0),
            core_city::NamedCitizen {
                id,
                placement_id: 1,
                slot: 0,
                name: core_city::name_for(id),
                age_days: 40 * 365,
            },
        );
        let mut runtime = CityRuntime::new(2026);
        runtime.sync(&[(1, 1)]);
        runtime.tick(10.0, 1, &[(1, 1)]);
        let before = pool.checksum();
        let outcome = tick_named_lifecycle(&mut pool, &mut runtime, 1.0);
        assert_eq!(outcome.tick.aged, 1);
        assert_eq!(outcome.tick.died, 0);
        assert_eq!(outcome.applied_deaths, 0);
        assert!(outcome.death_events.is_empty());
        assert_ne!(outcome.checksum, before);
    }

    #[test]
    fn named_lifecycle_tick_no_op_preserves_checksums_and_events() {
        let mut pool = NamedCitizens::new(2026);
        let id = core_city::citizen_id(2026, 1, 0);
        pool.citizens.insert(
            (1, 0),
            core_city::NamedCitizen {
                id,
                placement_id: 1,
                slot: 0,
                name: core_city::name_for(id),
                age_days: 40 * 365,
            },
        );
        let mut runtime = CityRuntime::new(2026);
        runtime.sync(&[(1, 1)]);
        runtime.tick(10.0, 1, &[(1, 1)]);
        let named_checksum = pool.checksum();
        let population_checksum = runtime.checksum();
        let outcome = tick_named_lifecycle(&mut pool, &mut runtime, 0.0);
        assert_eq!(outcome.tick, LifecycleTick::default());
        assert_eq!(outcome.applied_deaths, 0);
        assert!(outcome.death_events.is_empty());
        assert_eq!(outcome.checksum, named_checksum);
        assert_eq!(runtime.checksum(), population_checksum);
    }

    #[test]
    fn named_citizens_reject_an_unsupported_version() {
        let mut save = NamedCitizens::new(2026).to_save();
        save.format_version = 99;
        let json = attach_named_citizens("{}", &save).expect("attach");
        assert!(named_from_world_json(&json, 2026).is_err());
    }

    #[test]
    fn palette_materials_have_distinct_display_colors() {
        let wall = voxel_material_color(EDITOR_WALL);
        let floor = voxel_material_color(EDITOR_FLOOR);
        let roof = voxel_material_color(EDITOR_CEILING);
        let window = voxel_material_color(EDITOR_WINDOW);
        let door = voxel_material_color(EDITOR_DOOR);
        for (left, right) in [
            (wall, floor),
            (wall, roof),
            (wall, window),
            (wall, door),
            (window, door),
        ] {
            assert_ne!(left, right);
        }
        assert!(door[0] < wall[0]);
        assert!(window[2] > wall[2]);
    }

    #[test]
    fn starter_blueprints_build_and_validate() {
        for building in core_city::starters() {
            let blueprint = build_starter_blueprint(building).expect("starter volume");
            assert!(blueprint.metadata.solid_voxels > 0);
            assert_eq!(
                blueprint.metadata.stats.resident_capacity,
                building.resident_capacity
            );
            assert_eq!(blueprint.metadata.stats.job_slots, building.job_slots);
            if let Some(structure) = blueprint.metadata.structure {
                structure
                    .validate_against_bounds(blueprint.metadata.bounds_m)
                    .expect("starter structure");
            }
        }
    }

    #[test]
    fn route_preview_reuses_the_cache() {
        let graph = RoadGraph::build(&[RoadSegment {
            id: 1,
            start: [0, 0],
            end: [100, 0],
            width_m: 6,
        }])
        .expect("graph");
        let mut state = RoutePreviewState {
            seed: 2026,
            graph_checksum: graph.checksum(),
            cache: RouteCache::new(8),
        };
        let first =
            route_preview_data(&mut state, &graph, [10.0, 0.0], [90.0, 0.0]).expect("first route");
        assert!(!first.cached);
        assert_eq!(first.misses, 1);
        assert!(!first.points.is_empty());
        let second =
            route_preview_data(&mut state, &graph, [10.0, 0.0], [90.0, 0.0]).expect("second route");
        assert!(second.cached);
        assert_eq!(second.hits, 1);
        assert_eq!(first.checksum, second.checksum);
    }

    fn priced_store() -> ConstructionStore {
        let mut store = ConstructionStore::new(8, 1_000_000);
        store
            .add_priced_site(1, 1_000, 1_000.0)
            .expect("priced site");
        store
    }

    #[test]
    fn settle_payments_pays_the_first_installment_once() {
        let mut store = priced_store();
        let mut economy = Economy::new(2026);
        let first = settle_construction_payments(&mut store, Some(&mut economy));
        assert_eq!(first.paid_sites, 1);
        assert!((first.paid_total - 30.0).abs() < 1e-9);
        assert!((store.payment_due(1)).abs() < 1e-9);
        assert!(!store.sites[&1].payment_stalled);
        let second = settle_construction_payments(&mut store, Some(&mut economy));
        assert_eq!(second.paid_sites, 0);
        assert!(second.paid_total.abs() < 1e-9);
        assert!((economy.construction_spent - 30.0).abs() < 1e-9);
    }

    #[test]
    fn settle_payments_stalls_when_unaffordable_and_resumes() {
        let mut store = priced_store();
        let mut economy = Economy::new(2026);
        economy.balance = 0.0;
        let report = settle_construction_payments(&mut store, Some(&mut economy));
        assert_eq!(report.payment_stalled, 1);
        assert!(store.sites[&1].payment_stalled);
        assert!((store.payment_due(1) - 30.0).abs() < 1e-9);
        economy.balance = 100.0;
        let resumed = settle_construction_payments(&mut store, Some(&mut economy));
        assert_eq!(resumed.paid_sites, 1);
        assert!(!store.sites[&1].payment_stalled);
        assert!((store.payment_due(1)).abs() < 1e-9);
    }

    #[test]
    fn settle_payments_without_treasury_pays_and_stalls_nothing() {
        let mut store = priced_store();
        let report = settle_construction_payments(&mut store, None);
        assert_eq!(report.paid_sites, 0);
        assert_eq!(report.payment_stalled, 0);
        assert!(!store.sites[&1].payment_stalled);
        assert!((store.payment_due(1) - 30.0).abs() < 1e-9);
    }

    #[test]
    fn settle_payments_orders_by_placement_id() {
        let mut store = ConstructionStore::new(8, 1_000_000);
        store.add_priced_site(2, 1_000, 1_000.0).expect("site 2");
        store.add_priced_site(1, 1_000, 1_000.0).expect("site 1");
        let mut economy = Economy::new(2026);
        economy.balance = 30.0;
        let report = settle_construction_payments(&mut store, Some(&mut economy));
        assert_eq!(report.paid_sites, 1);
        assert_eq!(report.payment_stalled, 1);
        assert!((store.payment_due(1)).abs() < 1e-9);
        assert!((store.payment_due(2) - 30.0).abs() < 1e-9);
    }

    #[test]
    fn zero_cost_legacy_sites_are_untouched() {
        let mut store = ConstructionStore::new(8, 1_000_000);
        store.add_site(1, 1_000).expect("legacy site");
        let mut economy = Economy::new(2026);
        let report = settle_construction_payments(&mut store, Some(&mut economy));
        assert_eq!(report.paid_sites, 0);
        assert_eq!(report.payment_stalled, 0);
        assert!(store.sites[&1].payment_due().abs() < 1e-9);
    }

    #[test]
    fn first_installment_is_three_percent_rounded_to_cents() {
        assert!((first_construction_payment(1_000.0) - 30.0).abs() < 1e-9);
        assert!((first_construction_payment(12_345.67) - 370.37).abs() < 1e-9);
        assert!(first_construction_payment(f64::NAN).abs() < 1e-9);
        assert!(first_construction_payment(-5.0).abs() < 1e-9);
    }

    #[test]
    fn failed_mark_paid_refunds_the_first_installment_exactly() {
        let mut economy = Economy::new(2026);
        let balance = economy.balance;
        let checksum = economy.checksum();
        // The store has no site with id 1, so the charge succeeds but
        // `mark_paid` cannot be applied; the helper must refund it.
        let mut store = ConstructionStore::new(8, 1_000_000);
        assert!(!settle_first_installment(&mut economy, &mut store, 1, 30.0));
        assert_eq!(economy.balance, balance);
        assert_eq!(economy.construction_spent, 0.0);
        assert_eq!(economy.checksum(), checksum);
        economy.validate().expect("restored treasury is valid");
    }

    #[test]
    fn settled_first_installment_marks_the_site_paid_once() {
        let mut economy = Economy::new(2026);
        let mut store = priced_store();
        assert!(settle_first_installment(&mut economy, &mut store, 1, 30.0));
        assert!((store.payment_due(1)).abs() < 1e-9);
        assert!((economy.balance - (STARTING_BALANCE - 30.0)).abs() < 1e-9);
        assert!((economy.construction_spent - 30.0).abs() < 1e-9);
    }

    fn unique_save_path(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("townsmith_atomic_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir.join("world.json")
    }

    #[test]
    fn atomic_save_writes_and_reads_back() {
        let path = unique_save_path("roundtrip");
        let rotated = write_text_atomic(&path, "{\"a\":1}").expect("write");
        assert!(!rotated);
        assert!(!sibling_with_suffix(&path, ".tmp").exists());
        let read = read_json_with_backup(&path).expect("read");
        assert!(!read.recovered_from_backup);
        assert_eq!(read.json, "{\"a\":1}");
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn atomic_save_rotates_a_valid_previous_file() {
        let path = unique_save_path("rotate");
        write_text_atomic(&path, "{\"a\":1}").expect("first");
        let rotated = write_text_atomic(&path, "{\"a\":2}").expect("second");
        assert!(rotated);
        let backup = sibling_with_suffix(&path, ".bak");
        assert_eq!(
            std::fs::read_to_string(&backup).expect("backup"),
            "{\"a\":1}"
        );
        let read = read_json_with_backup(&path).expect("read");
        assert!(!read.recovered_from_backup);
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn atomic_save_recovers_a_corrupt_primary_from_backup() {
        let path = unique_save_path("recover");
        write_text_atomic(&path, "{\"a\":1}").expect("first");
        write_text_atomic(&path, "{\"a\":2}").expect("second");
        std::fs::write(&path, "not json").expect("corrupt primary");
        let read = read_json_with_backup(&path).expect("recover");
        assert!(read.recovered_from_backup);
        assert_eq!(read.json, "{\"a\":1}");
        assert!(read.backup_checksum != 0);
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn atomic_save_rejects_two_corrupt_files() {
        let path = unique_save_path("corrupt");
        std::fs::write(&path, "not json").expect("primary");
        std::fs::write(sibling_with_suffix(&path, ".bak"), "also not json").expect("backup");
        assert!(read_json_with_backup(&path).is_err());
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn atomic_save_rejects_a_missing_directory() {
        let path = std::env::temp_dir()
            .join("townsmith_atomic_missing_dir")
            .join("world.json");
        assert!(write_text_atomic(&path, "{}").is_err());
        assert!(read_json_with_backup(&path).is_err());
    }

    #[test]
    fn construction_lead_days_is_zero_only_for_developer() {
        assert_eq!(
            construction_lead_days(ConstructionTimeScale::DeveloperSeconds, 5.0),
            0.0
        );
        let campaign = ConstructionTimeScale::campaign_default();
        assert_eq!(construction_lead_days(campaign, 1.5), 1.5);
        assert_eq!(construction_lead_days(campaign, 0.0), 0.0);
    }

    #[test]
    fn infrastructure_initial_due_is_full_on_developer_and_half_on_campaign() {
        let cost = UtilityKind::Water.construction_cost();
        assert!(cost > 0.0);
        let developer = construction_lead_days(ConstructionTimeScale::DeveloperSeconds, 5.0);
        assert!((infrastructure_initial_due(cost, developer) - cost).abs() < 1e-9);
        let campaign = construction_lead_days(ConstructionTimeScale::campaign_default(), 5.0);
        assert_eq!(campaign, 5.0);
        assert!((infrastructure_initial_due(cost, campaign) - cost * 0.5).abs() < 1e-9);
        assert!((infrastructure_initial_due(cost, 0.0) - cost).abs() < 1e-9);
    }

    #[test]
    fn infrastructure_payment_reports_the_charged_installment() {
        let cost = UtilityKind::Water.construction_cost();
        let developer = infrastructure_payment(cost, cost);
        assert!(developer.charged);
        assert!((developer.cost_total - cost).abs() < 1e-9);
        assert!((developer.cost_paid - cost).abs() < 1e-9);
        assert!(developer.payment_due.abs() < 1e-9);

        let campaign = infrastructure_payment(cost, cost * 0.5);
        assert!(campaign.charged);
        assert!((campaign.cost_paid - cost * 0.5).abs() < 1e-9);
        assert!((campaign.payment_due - cost * 0.5).abs() < 1e-9);

        // A replacement keeps prior progress, so only the remainder is due.
        let replacement = infrastructure_payment(cost, cost + 10.0);
        assert!(replacement.charged);
        assert!(replacement.payment_due.abs() < 1e-9);
    }

    #[test]
    fn economy_for_seed_opens_a_missing_treasury() {
        let mut economy = None;
        let (treasury, created) = economy_for_seed(&mut economy, 2026).expect("treasury");
        assert!(created);
        assert_eq!(treasury.seed, 2026);
        assert!((treasury.balance - STARTING_BALANCE).abs() < 1e-9);
        assert_eq!(economy.as_ref().expect("open").seed, 2026);
    }

    #[test]
    fn economy_for_seed_reuses_an_existing_treasury() {
        let mut economy = Some(Economy::new(2026));
        economy.as_mut().expect("treasury").charge_construction(7.0);
        let (treasury, created) = economy_for_seed(&mut economy, 2026).expect("treasury");
        assert!(!created);
        assert!((treasury.balance - (STARTING_BALANCE - 7.0)).abs() < 1e-9);
    }

    #[test]
    fn economy_for_seed_rejects_a_foreign_seed() {
        let mut economy = Some(Economy::new(1));
        let (code, _) = economy_for_seed(&mut economy, 2).expect_err("foreign seed");
        assert_eq!(code, "seed_mismatch");
        let kept = economy.as_ref().expect("treasury stays open");
        assert_eq!(kept.seed, 1);
        assert!((kept.balance - STARTING_BALANCE).abs() < 1e-9);
    }

    #[test]
    fn charge_seed_treasury_charges_a_fresh_treasury() {
        let mut economy = None;
        let created = {
            let (_, created) = economy_for_seed(&mut economy, 2026).expect("treasury");
            created
        };
        assert!(created);
        charge_seed_treasury(&mut economy, created, 30.0, "utility site").expect("charge");
        let treasury = economy.as_ref().expect("treasury");
        assert_eq!(treasury.seed, 2026);
        assert!((treasury.balance - (STARTING_BALANCE - 30.0)).abs() < 1e-9);
        assert!((treasury.construction_spent - 30.0).abs() < 1e-9);
    }

    #[test]
    fn charge_seed_treasury_rolls_back_a_created_treasury() {
        let mut economy = None;
        let created = {
            let (_, created) = economy_for_seed(&mut economy, 2026).expect("treasury");
            created
        };
        assert!(created);
        let (code, _) = charge_seed_treasury(
            &mut economy,
            created,
            STARTING_BALANCE + 1.0,
            "utility site",
        )
        .expect_err("unaffordable");
        assert_eq!(code, "insufficient_funds");
        assert!(economy.is_none());
    }

    #[test]
    fn failed_charge_keeps_a_preexisting_treasury() {
        let mut economy = Some(Economy::new(2026));
        economy.as_mut().expect("treasury").balance = 10.0;
        let created = {
            let (_, created) = economy_for_seed(&mut economy, 2026).expect("treasury");
            created
        };
        assert!(!created);
        let (code, _) = charge_seed_treasury(&mut economy, created, 50.0, "service site")
            .expect_err("unaffordable");
        assert_eq!(code, "insufficient_funds");
        let treasury = economy.as_ref().expect("pre-existing treasury stays open");
        assert_eq!(treasury.seed, 2026);
        assert!((treasury.balance - 10.0).abs() < 1e-9);
    }

    #[test]
    fn failed_infrastructure_add_restores_a_missing_treasury() {
        let mut economy = None;
        let created = {
            let (_, created) = economy_for_seed(&mut economy, 2026).expect("treasury");
            created
        };
        assert!(created);
        // The caller reaches this rollback when the site add fails before the
        // charge, so the failed call leaves no treasury (and no seed pin).
        rollback_created_treasury(&mut economy, created);
        assert!(economy.is_none());
    }

    #[test]
    fn drop_joins_workers_without_hanging() {
        let bridge = TerrainBridge::new();
        drop(bridge);
    }
}
