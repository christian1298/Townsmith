//! Minimal Godot 4 bridge for Phase 0.
//!
//! The bridge deliberately exposes a coarse API: it builds meshes in the
//! engine-agnostic Rust core and hands them to Godot as `ArrayMesh` resources.
//! Terrain generation and meshing run on Rust worker threads; all Godot
//! resource creation stays on the main thread.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use core_city::player::{Player, PlayerError, PlayerId, PlayerRegistry};
use godot::builtin::{
    PackedColorArray, PackedFloat32Array, PackedInt32Array, PackedVector2Array, PackedVector3Array,
    VarArray, Variant,
};
use godot::classes::mesh::{ArrayType, PrimitiveType};
use godot::classes::{ArrayMesh, SurfaceTool};
use godot::prelude::*;

use core_blueprint::{
    load_tsbp, load_tsbp_with_dependency_issues, save_tsbp_with_hash, Blueprint, BlueprintError,
    BlueprintLibrary, BlueprintLibraryError, BlueprintMetadata, BlueprintStats, DependencyReason,
    LibraryInsert, LibrarySummary, PropRecord, RoomRecord,
};
use core_city::named::{age_years_from_days, slot_is_employed, MAX_AGE_DAYS};
use core_city::{
    advise, compose_tax_multiplier, diff_selection, evaluate_tutorial, first_construction_payment,
    heatmap_grid, normalize_rotation, policy_placement_denial, settle_first_installment, starter,
    structure_age_error, summarize, summarize_district, summarize_heatmap, AdvisorInput,
    AdvisorReport, Age, AgeState, CitizenDeath, CitizenMotion, CitizenPosition, CityLayout,
    CityRuntime, CitySummary, CityTier, ConstructionSite, ConstructionStore, ConstructionTimeScale,
    ContentPack, ContentRegistry, DistrictHeatmap, DistrictPolicy, DistrictPolicyError,
    DistrictPolicyRequest, DistrictPolicyStore, DistrictSummary, Economy, Home, Household,
    IndustryKind, IndustryStore, InfrastructureCosts, LifecycleTick, Lot, LotError, LotRequest,
    LotStore, MaterializationBudget, MaterializationSelection, MaterializedCitizen, MotionLeg,
    NamedCitizens, NamedSave, PackError, Placement, PlacementError, PlacementRequest,
    PlacementStore, PolicyPlacementDenial, PolicyServiceWeight, PolicyTaxWeights, ResearchStore,
    RoadError, RoadGraph, RoadRequest, RoadSegment, RoadStore, RouteCache, ServiceConsumer,
    ServiceKind, ServiceReport, ServiceStore, ServiceTier, Settlement, SettlementError,
    SettlementRequest, SettlementRole, SettlementStore, TaxLocation, TechTree, Trade, TradePool,
    TutorialInput, UtilityConsumer, UtilityKind, UtilityReport, UtilityStore, Workplace, Zone,
    DEFAULT_LOAN_RATE_PER_DAY, DEFAULT_LOAN_TERM_DAYS, DEFAULT_ROUTE_CACHE_CAPACITY,
    MAX_HEATMAP_CELLS, MAX_HEATMAP_CELL_M, MAX_LOANS, MAX_TAX_COMPOSITION_SETTLEMENT_RATE,
    MAX_TOTAL_DEBT, MAX_WORLD_COORD_M, MIN_HEATMAP_CELL_M, RESEARCH_PER_RESIDENT_PER_DAY,
    STARTING_BALANCE, TAX_COMPOSITION_CAP,
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

mod content_identity;
mod content_registry_ui;
mod placement_ui;

use content_identity::{
    stamp_world_content, verify_world_content, verify_world_content_inspecting,
    ActiveContentIdentity, ContentIdentityError, WorldContentIdentity,
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

/// Bridge-owned runtime content registry.
///
/// The slot starts with the bundled base content
/// ([`ContentRegistry::base`]) and never touches the filesystem until
/// [`Self::load_dir`] is called explicitly. A load validates one local mod
/// directory through [`ContentPack::from_dir`] and merges it after base
/// content; the live registry is replaced only after every check passes, so a
/// failed load leaves the previous registry untouched.
///
/// The slot is bridge state only: no content definition is embedded in
/// [`PlacementStore`] or the world save. D-034 records only a summary
/// identity in the top-level world JSON; [`Self::active_identity`] feeds that
/// summary. Registry, pack, and base checksums are unchanged by a world save
/// or load.
struct RuntimeContent {
    registry: Option<ContentRegistry>,
    pack: Option<RuntimePackInfo>,
    /// Stable checksum of the bundled base registry captured at creation.
    base_checksum: Option<u64>,
    base_error: Option<String>,
}

/// Identity of the one pack merged over the base content.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimePackInfo {
    /// Manifest ID, for example `example:copper_age`.
    id: String,
    /// Content namespace owned by the manifest.
    namespace: String,
    /// Stable pack checksum from [`ContentPack::checksum`].
    checksum: u64,
}

/// Read-only counts and identity for the live runtime registry.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ContentSummary {
    format_version: u32,
    materials: usize,
    rooms: usize,
    props: usize,
    total: usize,
    packs: usize,
    checksum: u64,
    /// Manifest ID of the live pack; empty when base only.
    pack_id: String,
    /// Content namespace owned by the live pack; empty when base only.
    namespace: String,
    /// Stable pack checksum; zero when base only.
    pack_checksum: u64,
    /// Stable bundled base checksum.
    base_checksum: u64,
}

/// Stable error from a runtime content operation.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ContentLoadError {
    /// Stable machine-readable code.
    code: &'static str,
    /// Human-readable message, safe to show in a mod manager.
    message: String,
}

/// Active content IDs used to resolve a `.tsbp` package's declared dependencies.
///
/// The view is derived from the live [`RuntimeContent`] slot, not from a package
/// or a save. Base-only registries expose the always-present `base` namespace.
/// One loaded pack adds its manifest ID and its namespace, so a package that
/// requires `mymod` matches an active `mymod:pack` and a package that requires
/// `mymod:pack` matches an active `mymod` (both sides split at the first `:`).
/// There is no discovery, cross-mod lookup, resource-pack registry, or download.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DependencyView {
    /// Trimmed active IDs, sorted and deduplicated.
    ids: Vec<String>,
}

impl DependencyView {
    /// The bundled base registry: only the always-present `base` namespace.
    fn base() -> Self {
        Self {
            ids: vec!["base".to_string()],
        }
    }

    /// Build the view from the live content slot.
    fn from_content(content: &RuntimeContent) -> Self {
        let mut view = Self::base();
        if let Some(pack) = content.pack.as_ref() {
            view.ids.push(pack.id.clone());
            view.ids.push(pack.namespace.clone());
        }
        view.ids.sort();
        view.ids.dedup();
        view
    }
}

impl Default for RuntimeContent {
    fn default() -> Self {
        Self::base()
    }
}

impl RuntimeContent {
    /// Open the slot on the bundled base registry without any disk access.
    fn base() -> Self {
        match ContentRegistry::base() {
            Ok(registry) => Self {
                base_checksum: Some(registry.checksum()),
                registry: Some(registry),
                pack: None,
                base_error: None,
            },
            Err(error) => Self {
                registry: None,
                pack: None,
                base_checksum: None,
                base_error: Some(error.to_string()),
            },
        }
    }

    /// Current registry counts, identity fields, and checksum.
    fn summary(&self) -> Result<ContentSummary, ContentLoadError> {
        let Some(registry) = self.registry.as_ref() else {
            return Err(self.base_error());
        };
        let base_checksum = self.base_checksum.ok_or_else(|| self.base_error())?;
        Ok(ContentSummary {
            format_version: registry.format_version(),
            materials: registry.material_count(),
            rooms: registry.room_count(),
            props: registry.prop_count(),
            total: registry.len(),
            packs: if self.pack.is_some() { 1 } else { 0 },
            checksum: registry.checksum(),
            pack_id: self
                .pack
                .as_ref()
                .map_or_else(String::new, |pack| pack.id.clone()),
            namespace: self
                .pack
                .as_ref()
                .map_or_else(String::new, |pack| pack.namespace.clone()),
            pack_checksum: self.pack.as_ref().map_or(0, |pack| pack.checksum),
            base_checksum,
        })
    }

    /// Summary identity for the world-save contract.
    ///
    /// Base-only registries report `packs = 0` and an empty pack ID. The
    /// merged registry checksum is the live [`ContentRegistry::checksum`];
    /// the base checksum is captured when the slot opens. This never reads a
    /// save and does not change any registry state.
    fn active_identity(&self) -> Result<ActiveContentIdentity, ContentLoadError> {
        let summary = self.summary()?;
        if summary.packs == 0 {
            Ok(ActiveContentIdentity::base(
                summary.checksum,
                summary.base_checksum,
            ))
        } else {
            Ok(ActiveContentIdentity::pack(
                summary.pack_id,
                summary.pack_checksum,
                summary.checksum,
                summary.base_checksum,
            ))
        }
    }

    /// Validate one absolute local mod directory and merge it after base.
    ///
    /// The path must be absolute, non-empty, and free of `..` components.
    /// [`ContentPack::from_dir`] then enforces the manifest content-path
    /// confinement, UTF-8, per-file, and total-size rules. The merged registry
    /// replaces the live one only after the pack and merge both validate.
    fn load_dir(&mut self, raw: &str) -> Result<RuntimePackInfo, ContentLoadError> {
        let dir = content_dir_from_arg(raw)?;
        if self.registry.is_none() {
            return Err(self.base_error());
        }
        let pack = ContentPack::from_dir(&dir).map_err(content_pack_error)?;
        let info = RuntimePackInfo {
            id: pack.manifest().id.clone(),
            namespace: pack.namespace().to_owned(),
            checksum: pack.checksum(),
        };
        let merged = pack.merged_with_base().map_err(content_pack_error)?;
        // Commit only after the pack and the merged registry both validate.
        self.registry = Some(merged);
        self.pack = Some(info.clone());
        self.base_error = None;
        Ok(info)
    }

    /// Drop the live pack and restore the bundled base registry.
    fn clear(&mut self) -> Result<(), ContentLoadError> {
        match ContentRegistry::base() {
            Ok(registry) => {
                self.base_checksum = Some(registry.checksum());
                self.registry = Some(registry);
                self.pack = None;
                self.base_error = None;
                Ok(())
            }
            Err(error) => {
                let message = error.to_string();
                self.registry = None;
                self.pack = None;
                self.base_checksum = None;
                self.base_error = Some(message.clone());
                Err(ContentLoadError {
                    code: "content_base_invalid",
                    message,
                })
            }
        }
    }

    fn base_error(&self) -> ContentLoadError {
        ContentLoadError {
            code: "content_base_invalid",
            message: self
                .base_error
                .clone()
                .unwrap_or_else(|| "bundled base content is unavailable".to_owned()),
        }
    }
}

/// Validate the bridge argument for [`RuntimeContent::load_dir`].
fn content_dir_from_arg(raw: &str) -> Result<PathBuf, ContentLoadError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ContentLoadError {
            code: "invalid_content_path",
            message: "content directory path is empty".to_owned(),
        });
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err(ContentLoadError {
            code: "invalid_content_path",
            message: "content directory path must be absolute".to_owned(),
        });
    }
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(ContentLoadError {
            code: "invalid_content_path",
            message: "content directory path must not contain '..'".to_owned(),
        });
    }
    Ok(path)
}

/// Map a pack failure to a stable bridge error code.
fn content_pack_error(error: PackError) -> ContentLoadError {
    let code = match &error {
        PackError::Manifest(_) => "content_invalid_manifest",
        PackError::Content(_) => "content_invalid_content",
        PackError::Base(_) => "content_base_invalid",
        PackError::Namespace { .. } => "content_namespace",
        PackError::CollisionWithBase { .. } => "content_duplicate_id",
        PackError::ReservedNamespace { .. } => "content_reserved_namespace",
        PackError::Merge(_) => "content_merge",
        PackError::MissingManifest { .. } => "content_missing_manifest",
        PackError::AmbiguousManifest { .. } => "content_ambiguous_manifest",
        PackError::NotADirectory { .. } => "content_not_a_directory",
        PackError::MissingFile { .. } => "content_missing_file",
        PackError::UnsafePath { .. } => "content_unsafe_path",
        PackError::PathEscape { .. } => "content_path_escape",
        PackError::NotAFile { .. } => "content_not_a_file",
        PackError::FileTooLarge { .. } => "content_file_too_large",
        PackError::PackTooLarge { .. } => "content_pack_too_large",
        PackError::NotUtf8 { .. } => "content_not_utf8",
        PackError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
            "content_directory_missing"
        }
        PackError::Io { .. } => "content_io",
    };
    ContentLoadError {
        code,
        message: error.to_string(),
    }
}

fn content_summary_dictionary(summary: &ContentSummary) -> VarDictionary {
    let mut result = VarDictionary::new();
    result.set("ok", true);
    result.set("format_version", i64::from(summary.format_version));
    result.set("materials", summary.materials as i64);
    result.set("rooms", summary.rooms as i64);
    result.set("props", summary.props as i64);
    result.set("total", summary.total as i64);
    result.set("packs", summary.packs as i64);
    result.set(
        "checksum",
        &GString::from(&format!("{:016x}", summary.checksum)),
    );
    result.set("pack_id", &GString::from(&summary.pack_id));
    result.set("namespace", &GString::from(&summary.namespace));
    result.set(
        "pack_checksum",
        &GString::from(&format!("{:016x}", summary.pack_checksum)),
    );
    result.set(
        "registry_checksum",
        &GString::from(&format!("{:016x}", summary.checksum)),
    );
    result.set(
        "base_checksum",
        &GString::from(&format!("{:016x}", summary.base_checksum)),
    );
    result
}

fn content_load_dictionary(
    path: &Path,
    pack: &RuntimePackInfo,
    summary: &ContentSummary,
) -> VarDictionary {
    let mut result = content_summary_dictionary(summary);
    result.set("path", &GString::from(path.to_string_lossy().as_ref()));
    result.set("pack_id", &GString::from(&pack.id));
    result.set("namespace", &GString::from(&pack.namespace));
    result.set(
        "pack_checksum",
        &GString::from(&format!("{:016x}", pack.checksum)),
    );
    result
}

/// Bridge-only content hash → package path map from the last portable load.
///
/// The map is populated only when `world_load_with_library` had to use a
/// library package because the recorded `blueprint_path` failed. It is never
/// serialized and does not participate in any placement or world checksum.
#[derive(Debug, Default)]
struct ResolvedBlueprintPaths {
    paths: BTreeMap<String, String>,
}

impl ResolvedBlueprintPaths {
    fn replace(&mut self, paths: BTreeMap<String, String>) {
        self.paths = paths;
    }

    fn clear(&mut self) {
        self.paths.clear();
    }

    /// Keep only hashes still referenced by the store.
    fn prune(&mut self, store: &PlacementStore) {
        let live: BTreeSet<&str> = store
            .placements
            .iter()
            .map(|placement| placement.blueprint_hash.as_str())
            .collect();
        self.paths.retain(|hash, _| live.contains(hash.as_str()));
    }

    /// The library path to expose for a placement, if it differs from the
    /// recorded path. A stale path (the file was deleted after loading) is
    /// returned as-is.
    fn path_for(&self, placement: &Placement) -> Option<&str> {
        self.paths
            .get(&placement.blueprint_hash)
            .map(String::as_str)
            .filter(|path| *path != placement.blueprint_path)
    }
}

/// Coarse bridge object used by the Godot shell.
#[derive(GodotClass)]
#[class(init, base=RefCounted)]
struct TownsmithCore {
    content: Mutex<RuntimeContent>,
    /// Verified D-034 identity of the open world. Bridge-only: committed after
    /// a durable content-bound save or a fully successful load, cleared by a
    /// base-only save/load and by `world_clear_placements`.
    world_content: Mutex<Option<WorldContentIdentity>>,
    /// Q-117: a content-bound world opened without its pack. Saves and
    /// content-dependent mutations are refused until a strict load succeeds.
    world_content_read_only: Mutex<bool>,
    terrain: Mutex<Option<TerrainBridge>>,
    editor: Mutex<Option<EditorState>>,
    world: Mutex<Option<PlacementStore>>,
    /// Content hash → library package path resolved during the last portable
    /// load. Bridge-only: never serialized and not part of any checksum.
    resolved_blueprint_paths: Mutex<ResolvedBlueprintPaths>,
    construction: Mutex<ConstructionStore>,
    roads: Mutex<Option<RoadStore>>,
    lots: Mutex<Option<LotStore>>,
    settlements: Mutex<Option<SettlementStore>>,
    /// Bridge-local player address book. Developer/local only: it is never
    /// written to a world save and is reset when the world seed changes.
    players: Mutex<LocalPlayers>,
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
            content: Mutex::new(RuntimeContent::base()),
            world_content: Mutex::new(None),
            world_content_read_only: Mutex::new(false),
            terrain: Mutex::new(None),
            editor: Mutex::new(None),
            world: Mutex::new(None),
            resolved_blueprint_paths: Mutex::new(ResolvedBlueprintPaths::default()),
            construction: Mutex::new(ConstructionStore::default()),
            roads: Mutex::new(None),
            lots: Mutex::new(None),
            settlements: Mutex::new(None),
            players: Mutex::new(LocalPlayers::default()),
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

    /// Read-only summary of the live runtime content registry.
    ///
    /// The registry starts at the bundled base content. D-034 records only
    /// the pack ID and checksums in a content-bound world save; no content
    /// definition or path is written. Returns:
    ///
    /// - `ok` (bool)
    /// - `format_version` (int)
    /// - `materials` / `rooms` / `props` / `total` (int counts)
    /// - `packs` (int; `0` for base only, `1` after `content_load_dir`)
    /// - `checksum` (String, merged registry checksum, 16 lowercase hex digits)
    /// - `pack_id` / `namespace` (String; empty for base only)
    /// - `pack_checksum` (String; `0000000000000000` for base only)
    /// - `registry_checksum` (String; same value as `checksum`)
    /// - `base_checksum` (String, bundled base registry checksum)
    ///
    /// On failure, returns `{ ok: false, error_code, error }`.
    #[func]
    fn content_registry_summary(&self) -> VarDictionary {
        let content = self
            .content
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match content.summary() {
            Ok(summary) => content_summary_dictionary(&summary),
            Err(error) => placement_simple_error(error.code, error.message),
        }
    }

    /// Load one local mod directory and merge it after base content.
    ///
    /// `path` must be an absolute local directory without `..`. The pack is
    /// validated by `ContentPack::from_dir` and merged with
    /// `merged_with_base`; the live registry is replaced only after every
    /// check passes, so a failed load leaves the previous registry untouched.
    /// Loading is explicit: there is no discovery, download, ZIP/Workshop,
    /// script, native-code, hot-reload, or cross-mod reference resolution.
    ///
    /// On success, returns the `content_registry_summary` keys plus `path`,
    /// `pack_id`, `namespace`, and `pack_checksum`. On failure, returns
    /// `{ ok: false, error_code, error }`.
    #[func]
    fn content_load_dir(&self, path: GString) -> VarDictionary {
        let raw = path.to_string();
        let mut content = self
            .content
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match content.load_dir(&raw) {
            Ok(pack) => match content.summary() {
                Ok(summary) => content_load_dictionary(Path::new(raw.trim()), &pack, &summary),
                Err(error) => placement_simple_error(error.code, error.message),
            },
            Err(error) => placement_simple_error(error.code, error.message),
        }
    }

    /// Drop the live mod pack and restore the bundled base registry.
    ///
    /// Returns `true` when the base registry loaded. This does not touch the
    /// world save, placements, blueprints, the recorded world content
    /// identity, or any existing checksum.
    #[func]
    fn content_registry_clear(&self) -> bool {
        self.content
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear()
            .is_ok()
    }

    /// Read-only report of one D-042 content-registry manifest file.
    ///
    /// `path` names an existing local JSON manifest of at most 1 MiB. The file
    /// is parsed and validated by `parse_content_registry_manifest`; this call
    /// never loads, selects, merges, or activates content, never touches the
    /// live registry, and never reads a pack directory. Returns:
    ///
    /// - `ok` (bool)
    /// - `format_version` (int)
    /// - `ids` (Array of `{id, kind, namespace, checksum}`), sorted by ID;
    ///   `kind` is `mod`/`resource_pack`, and an absent namespace or checksum
    ///   is `null`
    /// - `implicit_ids` (Array of String; `base` plus every manifest ID)
    /// - `checksum` (String, 16 lowercase hex digits over the sorted entries)
    ///
    /// On failure, returns `{ ok: false, error_code, error }` with a stable
    /// `content.*` code or `content.registry_io`. The path and raw file
    /// contents are never echoed.
    #[func]
    fn content_registry_manifest(&self, path: GString) -> VarDictionary {
        match content_registry_ui::content_registry_manifest_from_arg(&path.to_string()) {
            Ok(result) => result,
            Err((code, message)) => placement_simple_error(code, message),
        }
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

    /// Validate a `.tsbp` site for the ghost preview without mutating.
    ///
    /// Applies the same district-policy gate as
    /// [`Self::world_place_blueprint`] after package validation, so a passing
    /// preview cannot be denied later on policy.
    #[func]
    fn world_validate_placement(
        &self,
        seed: i64,
        path: GString,
        x: f64,
        z: f64,
        rotation: i64,
    ) -> VarDictionary {
        let active_ids = {
            let content = self
                .content
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            DependencyView::from_content(&content).ids
        };
        let active_refs: Vec<&str> = active_ids.iter().map(String::as_str).collect();
        let (seed, request) =
            match world_placement_request(seed, &path.to_string(), x, z, rotation, &active_refs) {
                Ok(value) => value,
                Err(error) => return error,
            };
        {
            let policies = self
                .policies
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(error) = placement_policy_error(
                policies.as_ref(),
                seed,
                [request.center_x, request.center_z],
                &request.blueprint_id,
            ) {
                return error;
            }
        }
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

    /// Summarize a local content-addressed blueprint library.
    ///
    /// `root_path` must be an absolute directory without `..`. The directory
    /// must already exist. The result is read-only and stateless; it is not
    /// part of the world save and no world, placement, or blueprint checksum
    /// changes. Returns:
    ///
    /// - `ok` (bool)
    /// - `count` (int)
    /// - `total_bytes` (int)
    /// - `checksum` (String, 16 lowercase hex digits)
    ///
    /// On failure, returns `{ ok: false, error_code, error }`.
    #[func]
    fn blueprint_library_summary(&self, root_path: GString) -> VarDictionary {
        match blueprint_library_summary_from_arg(&root_path.to_string()) {
            Ok(summary) => blueprint_library_summary_dictionary(&summary),
            Err((code, message)) => placement_simple_error(code, message),
        }
    }

    /// Enumerate a local content-addressed blueprint library for the UI.
    ///
    /// `root_path` follows [`Self::blueprint_library_summary`]. The result is
    /// read-only and stateless: `ok`, library summary fields, `entry_count`,
    /// `skipped`, and `entries[{hash, id, name, bytes, bounds_m,
    /// footprint_m}]`, sorted by hash. A package that cannot be revalidated is
    /// skipped, not returned as a partially trusted entry. No world,
    /// placement, or blueprint checksum changes.
    #[func]
    fn blueprint_library_entries(&self, root_path: GString) -> VarDictionary {
        match placement_ui::blueprint_library_entries_from_arg(&root_path.to_string()) {
            Ok(entries) => entries,
            Err((code, message)) => placement_simple_error(code, message),
        }
    }

    /// Insert one local `.tsbp` package into a blueprint library.
    ///
    /// `root_path` must be an absolute existing directory without `..`.
    /// `tsbp_path` must be an absolute `.tsbp` file without `..`. The package
    /// is validated by the existing loader and stored by its BLAKE3 content
    /// hash under `<root>/<first-two-hex>/<hash>.tsbp`. Identical bytes under
    /// an existing hash are a no-op; different bytes under the same hash are
    /// rejected. A failed insert leaves the library unchanged.
    ///
    /// This is local storage only: no discovery, download, ZIP/Workshop
    /// packaging, script execution, or world-save reference is added.
    ///
    /// On success, returns `ok`, `hash`, `path`, `inserted`, `count`,
    /// `total_bytes`, and `checksum`. On failure, returns
    /// `{ ok: false, error_code, error }`.
    #[func]
    fn blueprint_library_insert(&self, root_path: GString, tsbp_path: GString) -> VarDictionary {
        match blueprint_library_insert_from_args(&root_path.to_string(), &tsbp_path.to_string()) {
            Ok((insert, summary)) => {
                let mut result = blueprint_library_summary_dictionary(&summary);
                result.set("hash", &GString::from(insert.hash.as_str()));
                result.set(
                    "path",
                    &GString::from(insert.path.to_string_lossy().as_ref()),
                );
                result.set("inserted", insert.inserted);
                result
            }
            Err((code, message)) => placement_simple_error(code, message),
        }
    }

    /// Resolve a content hash in a local blueprint library.
    ///
    /// `root_path` must be an absolute existing directory without `..`.
    /// `hash` must be canonical `blake3:<64 lowercase hex>`. A valid but
    /// missing hash returns `ok = true`, `exists = false`, and an empty
    /// `path`. A malformed hash, unsafe path, or unreadable library returns
    /// `{ ok: false, error_code, error }`. This is a read-only local lookup;
    /// the world save still stores absolute package paths.
    #[func]
    fn blueprint_library_resolve(&self, root_path: GString, hash: GString) -> VarDictionary {
        match blueprint_library_resolve_from_args(&root_path.to_string(), &hash.to_string()) {
            Ok(Some(path)) => {
                let mut result = VarDictionary::new();
                result.set("ok", true);
                result.set("path", &GString::from(path.to_string_lossy().as_ref()));
                result.set("exists", true);
                result
            }
            Ok(None) => {
                let mut result = VarDictionary::new();
                result.set("ok", true);
                result.set("path", &GString::from(""));
                result.set("exists", false);
                result
            }
            Err((code, message)) => placement_simple_error(code, message),
        }
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
        let active_ids = {
            let content = self
                .content
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            DependencyView::from_content(&content).ids
        };
        let active_refs: Vec<&str> = active_ids.iter().map(String::as_str).collect();
        let (seed, request) =
            match world_placement_request(seed, &path.to_string(), x, z, rotation, &active_refs) {
                Ok(value) => value,
                Err(error) => return error,
            };
        {
            let policies = self
                .policies
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(error) = placement_policy_error(
                policies.as_ref(),
                seed,
                [request.center_x, request.center_z],
                &request.blueprint_id,
            ) {
                return error;
            }
        }
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
    ///
    /// A placement carrying a content-addressed library reference includes
    /// `blueprint_ref`. A placement whose package was resolved from the local
    /// library during the last [`Self::world_load_with_library`] also carries
    /// `resolved_blueprint_path`. The recorded `blueprint_path` is unchanged
    /// and remains the save value.
    #[func]
    fn world_placements(&self) -> VarDictionary {
        let guard = self.world.lock().unwrap_or_else(|error| error.into_inner());
        let resolved = self
            .resolved_blueprint_paths
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut result = VarDictionary::new();
        result.set("ok", true);
        match guard.as_ref() {
            Some(store) => {
                result.set("open", true);
                let mut placements: Array<VarDictionary> = Array::new();
                for placement in &store.placements {
                    placements.push(&placement_to_dictionary_with_resolved(
                        placement,
                        resolved.path_for(placement),
                    ));
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

    /// Read-only page of settlements ordered by ascending id.
    ///
    /// `after_id` is an exclusive cursor (pass the previous page's
    /// `next_after`). `limit` is clamped to 1–256; `0` means every remaining
    /// record. Never mutates the store or exposes a filesystem path.
    #[func]
    fn settlements_page(&self, after_id: i64, limit: i64) -> VarDictionary {
        let (after_id, limit) = match settlement_page_args(after_id, limit) {
            Ok(args) => args,
            Err((error_code, error)) => {
                let mut result = VarDictionary::new();
                result.set("ok", false);
                result.set("error_code", error_code);
                result.set("error", &GString::from(error.as_str()));
                return result;
            }
        };
        let store = self
            .settlements
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut result = VarDictionary::new();
        result.set("ok", true);
        match store.as_ref() {
            Some(store) => {
                let page = settlement_page(store.settlements(), after_id, limit);
                let mut settlements: Array<VarDictionary> = Array::new();
                for settlement in &page.settlements {
                    settlements.push(&settlement_to_dictionary(settlement));
                }
                result.set("open", true);
                result.set("seed", store.seed as i64);
                result.set("count", page.settlements.len() as i64);
                result.set("matched", page.matched as i64);
                result.set("next_after", page.next_after as i64);
                result.set("has_more", page.has_more);
                result.set(
                    "checksum",
                    &GString::from(&format!("{:016x}", store.checksum())),
                );
                result.set("settlements", &settlements.to_variant());
            }
            None => {
                result.set("open", false);
                result.set("seed", 0);
                result.set("count", 0);
                result.set("matched", 0);
                result.set("next_after", 0);
                result.set("has_more", false);
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

    /// Rename a settlement as its current owner.
    #[func]
    fn settlement_rename(&self, id: i64, name: GString, expected_revision: i64) -> VarDictionary {
        let (id, expected_revision) = match settlement_rename_args(id, expected_revision) {
            Ok(args) => args,
            Err((code, message)) => return placement_simple_error(code, message),
        };
        let mut store = self
            .settlements
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(store) = store.as_mut() else {
            return settlement_error_result(&SettlementError::Unknown { id });
        };
        match settlement_rename_in_store(store, id, &name.to_string(), expected_revision) {
            Ok(settlement) => {
                let mut result = VarDictionary::new();
                result.set("ok", true);
                result.set(
                    "settlement",
                    &settlement_to_dictionary(&settlement).to_variant(),
                );
                result.set(
                    "checksum",
                    &GString::from(&format!("{:016x}", store.checksum())),
                );
                result
            }
            Err((code, message)) => placement_simple_error(code, message),
        }
    }

    /// Claim an unowned settlement for `player_id`.
    #[func]
    fn settlement_claim(&self, id: i64, player_id: i64, expected_revision: i64) -> VarDictionary {
        let (id, expected_revision) = match settlement_mutation_args(id, expected_revision) {
            Ok(args) => args,
            Err((code, message)) => return placement_simple_error(code, message),
        };
        let player = match settlement_player_arg(player_id) {
            Ok(player) => player,
            Err((code, message)) => return placement_simple_error(code, message),
        };
        let mut guard = self
            .settlements
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(store) = guard.as_mut() else {
            return settlement_error_result(&SettlementError::Unknown { id });
        };
        let outcome = settlement_claim_in_store(store, id, player, expected_revision);
        settlement_mutation_payload(store, outcome)
    }

    /// Invite `player_id` as its current owner.
    #[func]
    fn settlement_invite(
        &self,
        id: i64,
        player_id: i64,
        role: GString,
        expected_revision: i64,
    ) -> VarDictionary {
        let (id, expected_revision) = match settlement_mutation_args(id, expected_revision) {
            Ok(args) => args,
            Err((code, message)) => return placement_simple_error(code, message),
        };
        let player = match settlement_player_arg(player_id) {
            Ok(player) => player,
            Err((code, message)) => return placement_simple_error(code, message),
        };
        let mut guard = self
            .settlements
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(store) = guard.as_mut() else {
            return settlement_error_result(&SettlementError::Unknown { id });
        };
        let outcome =
            settlement_invite_in_store(store, id, player, &role.to_string(), expected_revision);
        settlement_mutation_payload(store, outcome)
    }

    /// Change a member's role as its current owner.
    #[func]
    fn settlement_set_role(
        &self,
        id: i64,
        player_id: i64,
        role: GString,
        expected_revision: i64,
    ) -> VarDictionary {
        let (id, expected_revision) = match settlement_mutation_args(id, expected_revision) {
            Ok(args) => args,
            Err((code, message)) => return placement_simple_error(code, message),
        };
        let player = match settlement_player_arg(player_id) {
            Ok(player) => player,
            Err((code, message)) => return placement_simple_error(code, message),
        };
        let mut guard = self
            .settlements
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(store) = guard.as_mut() else {
            return settlement_error_result(&SettlementError::Unknown { id });
        };
        let outcome =
            settlement_set_role_in_store(store, id, player, &role.to_string(), expected_revision);
        settlement_mutation_payload(store, outcome)
    }

    /// Remove a member as its current owner.
    #[func]
    fn settlement_remove_member(
        &self,
        id: i64,
        player_id: i64,
        expected_revision: i64,
    ) -> VarDictionary {
        let (id, expected_revision) = match settlement_mutation_args(id, expected_revision) {
            Ok(args) => args,
            Err((code, message)) => return placement_simple_error(code, message),
        };
        let player = match settlement_player_arg(player_id) {
            Ok(player) => player,
            Err((code, message)) => return placement_simple_error(code, message),
        };
        let mut guard = self
            .settlements
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(store) = guard.as_mut() else {
            return settlement_error_result(&SettlementError::Unknown { id });
        };
        let outcome = settlement_remove_member_in_store(store, id, player, expected_revision);
        settlement_mutation_payload(store, outcome)
    }

    /// Transfer ownership to `player_id` as its current owner.
    #[func]
    fn settlement_transfer(
        &self,
        id: i64,
        player_id: i64,
        expected_revision: i64,
    ) -> VarDictionary {
        let (id, expected_revision) = match settlement_mutation_args(id, expected_revision) {
            Ok(args) => args,
            Err((code, message)) => return placement_simple_error(code, message),
        };
        let player = match settlement_player_arg(player_id) {
            Ok(player) => player,
            Err((code, message)) => return placement_simple_error(code, message),
        };
        let mut guard = self
            .settlements
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(store) = guard.as_mut() else {
            return settlement_error_result(&SettlementError::Unknown { id });
        };
        let outcome = settlement_transfer_in_store(store, id, player, expected_revision);
        settlement_mutation_payload(store, outcome)
    }

    /// Admin override: clear a settlement's owner and members without an actor.
    ///
    /// This mirrors the core/server Admin release and is local/developer-only:
    /// the bridge has no capability gate, so the standalone settlement editor
    /// is its only caller. The revision guard still applies.
    #[func]
    fn settlement_release(&self, id: i64, expected_revision: i64) -> VarDictionary {
        let (id, expected_revision) = match settlement_mutation_args(id, expected_revision) {
            Ok(args) => args,
            Err((code, message)) => return placement_simple_error(code, message),
        };
        let mut guard = self
            .settlements
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(store) = guard.as_mut() else {
            return settlement_error_result(&SettlementError::Unknown { id });
        };
        let outcome = settlement_release_in_store(store, id, expected_revision);
        settlement_mutation_payload(store, outcome)
    }

    /// Set the settlement tax rate as its current owner.
    #[func]
    fn settlement_set_tax(&self, id: i64, rate: i64) -> VarDictionary {
        let Ok(id) = u64::try_from(id) else {
            return placement_simple_error("invalid_id", "settlement id must be non-negative");
        };
        let Ok(rate) = u16::try_from(rate) else {
            return placement_simple_error(
                "settlement.tax_out_of_range",
                "settlement tax rate must fit 16 bits",
            );
        };
        let mut guard = self
            .settlements
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(store) = guard.as_mut() else {
            return settlement_error_result(&SettlementError::Unknown { id });
        };
        let outcome = settlement_set_tax_in_store(store, id, rate);
        settlement_mutation_payload(store, outcome)
    }

    /// Read-only settlement-tax projection for one settlement rate.
    ///
    /// This previews the proposed D-039 option-A composition at one neutral
    /// base location (`TaxLocation::new(1.0, rate)`): the result is
    /// `min(8.0, 1 + rate / 100)`. It never reads or writes the treasury, the
    /// settlement store, the world save, content, or a path. D-039 remains
    /// Proposed; the dashboard uses this only to render a preview line.
    #[func]
    fn tax_projection(&self, rate: i64) -> VarDictionary {
        tax_projection_payload(rate)
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

    /// Bridge player registry for the current world seed.
    ///
    /// The seed follows the first open world, settlement, road, or lot layer;
    /// those layers are kept in lockstep, so a disagreement is reported as
    /// `player.seed_mismatch` instead of picking one. With no layer open the
    /// seed is [`BRIDGE_PLAYER_DEFAULT_SEED`]. Every bridge player call
    /// re-checks the seed: when it changes, the previous names are dropped and
    /// ids restart at 1. No name is carried across seeds, and nothing is
    /// persisted.
    fn local_players(&self) -> Result<MutexGuard<'_, LocalPlayers>, (&'static str, &'static str)> {
        let world = self
            .world
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .map(|store| store.seed);
        let settlements = self
            .settlements
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .map(|store| store.seed);
        let roads = self
            .roads
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .map(|store| store.seed);
        let lots = self
            .lots
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .map(|store| store.seed);
        let seed = bridge_player_seed([world, settlements, roads, lots])?;
        let mut players = self
            .players
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        player_registry_sync_seed(&mut players.0, seed);
        Ok(players)
    }

    /// Register a local bridge player, or return the existing record.
    ///
    /// `name` is trimmed, must be 1–32 characters without control characters,
    /// and is matched case-insensitively. Registering an existing name again is
    /// idempotent and returns its original id and display casing. Ids are
    /// stable per bridge seed and start at 1.
    ///
    /// Returns `{ ok, player: { id, name }, count, checksum }` on success.
    /// Errors are `player.invalid_name`, `player.limit` (256 players), and
    /// `player.seed_mismatch` when open world layers disagree. This is a local
    /// developer address book, not server authentication, and it is not saved.
    #[func]
    fn players_register(&self, name: GString) -> VarDictionary {
        let mut players = match self.local_players() {
            Ok(players) => players,
            Err((code, message)) => return placement_simple_error(code, message),
        };
        match players_register_in_store(&mut players.0, &name.to_string()) {
            Ok(player) => players_register_payload(&players.0, &player),
            Err((code, message)) => placement_simple_error(code, message),
        }
    }

    /// List every local bridge player in ascending id order.
    ///
    /// Returns `{ ok, count, checksum, players: [{ id, name }] }`. The
    /// checksum is the [`PlayerRegistry`] FNV-1a value as 16 lowercase hex
    /// digits. Read-only: it never registers a name.
    #[func]
    fn players_list(&self) -> VarDictionary {
        match self.local_players() {
            Ok(players) => players_list_payload(&players.0),
            Err((code, message)) => placement_simple_error(code, message),
        }
    }

    /// Find a local bridge player by display name.
    ///
    /// The name is trimmed and matched case-insensitively. Returns
    /// `{ ok, found, player: { id, name } or null }`; a missing name is not an
    /// error.
    #[func]
    fn players_find(&self, name: GString) -> VarDictionary {
        match self.local_players() {
            Ok(players) => players_find_payload(&players.0, &name.to_string()),
            Err((code, message)) => placement_simple_error(code, message),
        }
    }

    /// Remove every placement. The seed and an empty store stay open.
    #[func]
    fn world_clear_placements(&self) -> bool {
        let mut guard = self.world.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(store) = guard.as_mut() {
            store.clear();
        }
        self.resolved_blueprint_paths
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
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
        *self
            .world_content
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        *self
            .world_content_read_only
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = false;
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
            if let Some(store) = guard.as_ref() {
                self.resolved_blueprint_paths
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .prune(store);
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
    /// a `seed_mismatch`. The standalone developer script
    /// `godot/policy_editor.gd` is the only UI consumer; no simulation,
    /// household, treasury, or economy code reads these values. The registry
    /// is persisted in the world save, but saving it applies no tax, funding,
    /// construction, or allowlist effect.
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
        // Resolve district tax and service-funding weights before taking the
        // economy lock. This block holds world -> construction -> policies ->
        // population -> services in the documented order (services are read
        // last and every store is released before the treasury lock).
        let (policy_weights, policy_service_weight, service_sites, service_upkeep) = {
            let placements = self.world.lock().unwrap_or_else(|error| error.into_inner());
            let construction = self
                .construction
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let policies = self
                .policies
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let population_guard = self
                .population
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let services = self
                .services
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let (homes, workplaces) = match placements.as_ref() {
                Some(placements) => (
                    completed_homes(placements, &construction)
                        .into_iter()
                        .map(|home| (home.id, home.center))
                        .collect(),
                    completed_workplaces(placements, &construction),
                ),
                None => (BTreeMap::new(), Vec::new()),
            };
            let policy_weights = match population_guard.as_ref() {
                Some(runtime) => policy_tax_weights(
                    policies.as_ref(),
                    seed,
                    placements.as_ref().map(|placements| placements.seed),
                    Some(runtime),
                    &homes,
                    &workplaces,
                    &runtime.staffing,
                ),
                None => PolicyTaxWeights::ONE,
            };
            let policy_service_weight = policy_service_funding_weight(
                policies.as_ref(),
                seed,
                placements.as_ref().map(|placements| placements.seed),
                population_guard.as_ref().map(|runtime| runtime.seed),
                services.as_ref(),
            );
            let (service_sites, service_upkeep) = services
                .as_ref()
                .filter(|services| services.seed == seed)
                .map_or((0, 0.0), |services| {
                    (
                        services.site_count() as u64,
                        services.total_upkeep_per_day(),
                    )
                });
            (
                policy_weights,
                policy_service_weight,
                service_sites,
                service_upkeep,
            )
        };
        // Read the utility site count and upkeep total before taking the
        // economy lock: the lock order is world -> ... -> economy ->
        // utilities/services.
        let (utility_sites, utility_upkeep) = self.utility_infrastructure_for_seed(seed);
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
        let tick = economy.tick_with_policy_weights(
            seconds,
            population,
            employed,
            buildings,
            infrastructure,
            policy_weights,
            policy_service_weight,
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
        result.set("policy_tax_multiplier_resident", policy_weights.resident());
        result.set("policy_tax_multiplier_business", policy_weights.business());
        result.set(
            "policy_service_funding_weight",
            policy_service_weight.value(),
        );
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
    ///
    /// With no live content pack the output keeps outer `format_version: 1`,
    /// writes no `content` member, and is byte-identical to the pre-D-034
    /// format. One live pack writes outer `format_version: 2` and a top-level
    /// `content` identity object after the whole JSON is assembled and before
    /// the durable write; the identity is committed to the live world only
    /// after that write succeeds. The result adds `content_bound`,
    /// `content_identity_status`, `content_pack_id`, `content_pack_checksum`,
    /// `content_registry_checksum`, and `content_format_version`.
    #[func]
    fn world_save(&self, path: GString) -> VarDictionary {
        self.world_save_impl(path, None)
    }

    /// Write the placement store and copy its packages into a local library.
    ///
    /// `library_root` must be an absolute existing directory without `.` or
    /// `..` (conventionally a `blueprints/` directory beside the save). Every
    /// unique placement package is validated and copied by content hash before
    /// the save is written. Each saved record also carries the canonical
    /// `tsbp:blake3:<64hex>` reference; the legacy `blueprint_path` stays as a
    /// fallback. A record whose path is missing can still be saved when its
    /// reference resolves in the library. The live store commits the
    /// referenced clone only after a durable write, so a later plain
    /// [`Self::world_save`] keeps the references.
    ///
    /// On failure, returns `{ ok: false, error_code, error }` and does not
    /// write the save or mutate the live store. Missing sources report
    /// `blueprint_library_missing_file`, invalid sources
    /// `blueprint_library_invalid_package`, and mismatched packages keep
    /// `blueprint_hash_mismatch` / `blueprint_metadata_mismatch`.
    #[func]
    fn world_save_with_library(&self, path: GString, library_root: GString) -> VarDictionary {
        let library = match blueprint_library_open(&library_root.to_string()) {
            Ok(library) => library,
            Err((code, message)) => return placement_simple_error(code, message),
        };
        self.world_save_impl(path, Some(&library))
    }

    fn world_save_impl(&self, path: GString, library: Option<&BlueprintLibrary>) -> VarDictionary {
        if *self
            .world_content_read_only
            .lock()
            .unwrap_or_else(|error| error.into_inner())
        {
            return content_identity_error_result(&content_read_only_error());
        }
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
        let Some(mut store) = store else {
            return placement_simple_error("no_world", "no world placement store is open");
        };
        // A library save stamps every record with its content address on the
        // saved clone. The live store commits this clone only after the write
        // is durable, so a failed save changes no live record.
        if library.is_some() {
            stamp_world_blueprint_references(&mut store);
        }
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
        // D-034: decide the content identity before the file is written. A
        // base-only world returns the assembled JSON untouched; a bound world
        // gets outer v2 and the one `content` object. Nothing is committed
        // until the write is durable.
        let active_content = {
            let content = self
                .content
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            match content.active_identity() {
                Ok(identity) => identity,
                Err(error) => return placement_simple_error(error.code, error.message),
            }
        };
        let recorded_content = self
            .world_content
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let content_stamp =
            match stamp_world_content(&json, &active_content, recorded_content.as_ref()) {
                Ok(stamp) => stamp,
                Err(error) => return content_identity_error_result(&error),
            };
        let content_commit = content_stamp.identity.clone();
        let content_bound = content_stamp.content_bound();
        let content_status = content_stamp.status_str();
        let content_format_version = content_stamp.format_version();
        let (content_pack_id, content_pack_checksum, content_registry_checksum) =
            match content_stamp.identity.as_ref() {
                Some(identity) => (
                    identity.pack_id.clone(),
                    format!("{:016x}", identity.pack_checksum),
                    format!("{:016x}", identity.registry_checksum),
                ),
                None => (
                    String::new(),
                    "0000000000000000".to_string(),
                    "0000000000000000".to_string(),
                ),
            };
        let json = content_stamp.json;
        if json.len() > MAX_WORLD_SAVE_BYTES as usize {
            return placement_simple_error(
                "save_too_large",
                format!(
                    "world save is {} bytes; the limit is {MAX_WORLD_SAVE_BYTES}",
                    json.len()
                ),
            );
        }
        let library_sync = match library {
            Some(library) => match sync_world_blueprints(&store, library) {
                Ok(sync) => Some(sync),
                Err((code, message)) => return placement_simple_error(code, message),
            },
            None => None,
        };
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
        // A library save stamps content addresses on the cloned records. Commit
        // that clone only after the durable write so a later plain save keeps
        // the references and `world_placements` reports them.
        if library.is_some() {
            let mut guard = self.world.lock().unwrap_or_else(|error| error.into_inner());
            if guard.as_ref().is_some_and(|live| live.seed == store.seed) {
                *guard = Some(store.clone());
            }
        }
        // D-034: the new or verified identity is committed only after the
        // write is durable. A base-only save clears it (reachable only when no
        // identity was recorded, since a bound world refuses that save).
        *self
            .world_content
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = content_commit;
        let (policy_count, policy_checksum) = policy_summary(policies.as_ref());
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("path", &GString::from(path.to_string_lossy().as_ref()));
        result.set("bytes", json.len() as i64);
        result.set("backup_rotated", backup_rotated);
        result.set("content_bound", content_bound);
        set_content_identity_fields(
            &mut result,
            content_status,
            &content_pack_id,
            &content_pack_checksum,
            &content_registry_checksum,
            content_format_version,
        );
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
        result.set("district_policies", policy_count as i64);
        result.set(
            "district_policy_checksum",
            &GString::from(&format!("{policy_checksum:016x}")),
        );
        result.set(
            "terrain_checksum",
            &GString::from(&format!("{deltas_checksum:016x}")),
        );
        if let Some(sync) = library_sync {
            result.set("blueprint_packages", sync.unique as i64);
            result.set("blueprint_packages_inserted", sync.inserted as i64);
            result.set("blueprint_packages_existing", sync.existing as i64);
            result.set("blueprint_library_packages", sync.library_count as i64);
            result.set(
                "blueprint_references",
                store
                    .placements
                    .iter()
                    .filter(|placement| placement.blueprint_ref.is_some())
                    .count() as i64,
            );
        }
        result
    }

    /// Read a world placement save for `expected_seed` and verify its packages.
    ///
    /// The content identity is checked immediately after the JSON is read and
    /// before terrain, stores, or packages are parsed. A v2 match is
    /// normalized to v1 in memory for the existing parsers and the file is
    /// not rewritten. The result adds `content_identity_status`,
    /// `content_pack_id`, `content_pack_checksum`, `content_registry_checksum`,
    /// and `content_format_version`.
    #[func]
    fn world_load(&self, path: GString, expected_seed: i64) -> VarDictionary {
        self.world_load_impl(path, expected_seed, None, false)
    }

    /// Load a world with an explicit missing-content inspection opt-in (Q-117).
    ///
    /// With `accept_missing_content = false` this is exactly
    /// [`Self::world_load`], including the fail-closed content identity check.
    /// With `true`, a content-bound save also loads when the live registry is
    /// base-only or holds a different pack; the result reports
    /// `content_read_only = true`, keeps the recorded identity opaque, and
    /// refuses every later save with `content_read_only`. Unknown content IDs
    /// are never substituted. There is no auto-download and no path rewrite.
    #[func]
    fn world_load_content_checked(
        &self,
        path: GString,
        expected_seed: i64,
        accept_missing_content: bool,
    ) -> VarDictionary {
        self.world_load_impl(path, expected_seed, None, accept_missing_content)
    }

    /// Read a world save, resolving placement packages from a local library.
    ///
    /// Load semantics are identical to [`Self::world_load`]. Resolution order
    /// per placement is: a `blueprint_ref` (library), the recorded
    /// `blueprint_path` (legacy/local), then the library by `blueprint_hash`.
    /// A reference is never treated as a path: without a library, a working
    /// recorded path is used and a failed one reports
    /// `blueprint_reference_requires_library`. A reference that resolves to an
    /// invalid or mismatched package fails closed. The save JSON is never
    /// rewritten. The result adds `resolved_references` and
    /// `legacy_path_fallbacks`.
    #[func]
    fn world_load_with_library(
        &self,
        path: GString,
        expected_seed: i64,
        library_root: GString,
    ) -> VarDictionary {
        let library = match blueprint_library_open(&library_root.to_string()) {
            Ok(library) => library,
            Err((code, message)) => return placement_simple_error(code, message),
        };
        self.world_load_impl(path, expected_seed, Some(&library), false)
    }

    fn world_load_impl(
        &self,
        path: GString,
        expected_seed: i64,
        library: Option<&BlueprintLibrary>,
        accept_missing_content: bool,
    ) -> VarDictionary {
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
        // D-034: verify the content identity immediately after reading,
        // before terrain, stores, or any `.tsbp` is parsed. A v2 match is
        // normalized to the v1 shape for the existing parsers; the file is
        // never rewritten here.
        let active_content = {
            let content = self
                .content
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            match content.active_identity() {
                Ok(identity) => identity,
                Err(error) => return placement_simple_error(error.code, error.message),
            }
        };
        let content_verification = if accept_missing_content {
            verify_world_content_inspecting(&json, &active_content)
        } else {
            verify_world_content(&json, &active_content)
        };
        let content_verification = match content_verification {
            Ok(verification) => verification,
            Err(error) => return content_identity_error_result(&error),
        };
        let content_commit = content_verification.identity.clone();
        let content_read_only = content_verification.read_only();
        let content_status = content_verification.status_str();
        let content_format_version = content_verification.format_version;
        let (content_pack_id, content_pack_checksum, content_registry_checksum) =
            match content_verification.identity.as_ref() {
                Some(identity) => (
                    identity.pack_id.clone(),
                    format!("{:016x}", identity.pack_checksum),
                    format!("{:016x}", identity.registry_checksum),
                ),
                None => (
                    String::new(),
                    "0000000000000000".to_string(),
                    "0000000000000000".to_string(),
                ),
            };
        let json = match content_verification.normalized_json {
            Some(normalized) => normalized,
            None => json,
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
        let LoadedWorldPlacements {
            solid_by_id,
            resolved_paths: resolved_blueprint_paths,
            resolved_references,
            legacy_path_fallbacks,
        } = match load_world_placement_packages(&store, library) {
            Ok(loaded) => loaded,
            Err((code, message)) => return placement_simple_error(code, message),
        };
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
        let mut resolved_guard = self
            .resolved_blueprint_paths
            .lock()
            .unwrap_or_else(|error| error.into_inner());
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
        // Plain `world_load` installs an empty map; a portable load installs
        // the fallback entries collected above. The map changes only after
        // every store and package check has passed.
        resolved_guard.replace(resolved_blueprint_paths);
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
        // D-034: commit the verified identity only after the whole load has
        // succeeded. Absent, legacy, and base-only loads clear it.
        *self
            .world_content
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = content_commit;
        *self
            .world_content_read_only
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = content_read_only;
        let mut result = VarDictionary::new();
        result.set("ok", true);
        result.set("path", &GString::from(path.to_string_lossy().as_ref()));
        result.set("recovered_from_backup", read.recovered_from_backup);
        set_content_identity_fields(
            &mut result,
            content_status,
            &content_pack_id,
            &content_pack_checksum,
            &content_registry_checksum,
            content_format_version,
        );
        result.set("content_read_only", content_read_only);
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
        if library.is_some() {
            result.set("resolved_references", resolved_references as i64);
            result.set("legacy_path_fallbacks", legacy_path_fallbacks as i64);
        }
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

/// Pure payload for [`TownsmithCore::tax_projection`].
///
/// Validates `rate` in whole percent, composes one neutral-base location, and
/// reports the effective multiplier plus whether the per-location cap applied.
/// The helper is deliberately stateless: no bridge lock, save, content, or
/// path is touched, so the dashboard preview cannot mutate the world.
fn tax_projection_value(rate: i64) -> Result<(u16, f64, bool), (&'static str, &'static str)> {
    let Ok(rate) = u16::try_from(rate) else {
        return Err((
            "tax.projection.rate_out_of_range",
            "settlement tax rate must be between 0 and 100",
        ));
    };
    if rate > MAX_TAX_COMPOSITION_SETTLEMENT_RATE {
        return Err((
            "tax.projection.rate_out_of_range",
            "settlement tax rate must be between 0 and 100",
        ));
    }
    let location = TaxLocation::new(1.0, rate).map_err(|error| {
        (
            error.code(),
            "settlement tax rate must be between 0 and 100",
        )
    })?;
    let combined_multiplier = compose_tax_multiplier(&[location]).map_err(|error| {
        (
            error.code(),
            "settlement tax rate must be between 0 and 100",
        )
    })?;
    Ok((
        rate,
        combined_multiplier,
        combined_multiplier >= TAX_COMPOSITION_CAP,
    ))
}

fn tax_projection_payload(rate: i64) -> VarDictionary {
    match tax_projection_value(rate) {
        Ok((rate, combined_multiplier, capped)) => {
            let mut result = VarDictionary::new();
            result.set("ok", true);
            result.set("rate", i64::from(rate));
            result.set("combined_multiplier", combined_multiplier);
            result.set("capped", capped);
            result
        }
        Err((code, message)) => placement_simple_error(code, message),
    }
}

/// Largest explicit page the bridge will serialize in one call.
const BRIDGE_SETTLEMENT_PAGE_LIMIT: u64 = 256;

/// Validated paging arguments: exclusive id cursor and optional page size.
///
/// `limit == 0` means "all remaining", which keeps the cursor usable for a
/// full read without inventing a sentinel beyond 256.
fn settlement_page_args(
    after_id: i64,
    limit: i64,
) -> Result<(u64, Option<usize>), (&'static str, String)> {
    let after_id = u64::try_from(after_id).map_err(|_| {
        (
            "invalid_page",
            format!("after_id must be a non-negative integer: {after_id}"),
        )
    })?;
    let limit = u64::try_from(limit).map_err(|_| {
        (
            "invalid_page",
            format!("limit must be non-negative: {limit}"),
        )
    })?;
    let limit = if limit == 0 {
        None
    } else {
        Some(
            usize::try_from(limit.min(BRIDGE_SETTLEMENT_PAGE_LIMIT))
                .expect("page limit is bounded to 256"),
        )
    };
    Ok((after_id, limit))
}

/// One read-only slice of a settlement store.
struct SettlementPage<'a> {
    /// Selected records in ascending id order.
    settlements: Vec<&'a Settlement>,
    /// Records strictly after the cursor, before the page cap.
    matched: usize,
    /// Last returned id, or 0 for an empty page.
    next_after: u64,
    /// Whether at least one record remains after this page.
    has_more: bool,
}

/// Select `id > after_id` in ascending order, capped by `limit`.
///
/// The store keeps records sorted by id, but sorting here keeps paging correct
/// if a future loader accepts records out of order.
fn settlement_page<'a>(
    settlements: &'a [Settlement],
    after_id: u64,
    limit: Option<usize>,
) -> SettlementPage<'a> {
    let mut remaining: Vec<&Settlement> = settlements
        .iter()
        .filter(|settlement| settlement.id > after_id)
        .collect();
    remaining.sort_by_key(|settlement| settlement.id);
    let matched = remaining.len();
    let page_len = limit.unwrap_or(matched).min(matched);
    remaining.truncate(page_len);
    let next_after = remaining.last().map_or(0, |settlement| settlement.id);
    let has_more = page_len < matched;
    SettlementPage {
        settlements: remaining,
        matched,
        next_after,
        has_more,
    }
}

fn settlement_error_result(error: &SettlementError) -> VarDictionary {
    placement_simple_error(error.code(), error.to_string())
}

/// Convert the Godot rename arguments to the core domain types.
fn settlement_rename_args(
    id: i64,
    expected_revision: i64,
) -> Result<(u64, u32), (&'static str, &'static str)> {
    let id = u64::try_from(id).map_err(|_| ("invalid_id", "settlement id must be non-negative"))?;
    let expected_revision = u32::try_from(expected_revision)
        .map_err(|_| ("invalid_revision", "settlement revision must fit 32 bits"))?;
    Ok((id, expected_revision))
}

/// Rename `id` as its current owner, without touching an unowned settlement.
///
/// The bridge has no player registry, so the local actor is the settlement's
/// own `owner_id`. This never transfers ownership or changes membership.
fn settlement_rename_in_store(
    store: &mut SettlementStore,
    id: u64,
    name: &str,
    expected_revision: u32,
) -> Result<Settlement, (&'static str, String)> {
    let Some(settlement) = store.get(id) else {
        let error = SettlementError::Unknown { id };
        return Err((error.code(), error.to_string()));
    };
    let Some(owner) = settlement.owner_id else {
        let error = SettlementError::Unowned { id };
        return Err((error.code(), error.to_string()));
    };
    store
        .rename(owner, id, name, expected_revision)
        .map_err(|error| (error.code(), error.to_string()))
}

/// Convert the Godot mutation arguments to the core domain types.
fn settlement_mutation_args(
    id: i64,
    expected_revision: i64,
) -> Result<(u64, u32), (&'static str, &'static str)> {
    let id = u64::try_from(id).map_err(|_| ("invalid_id", "settlement id must be non-negative"))?;
    let expected_revision = u32::try_from(expected_revision)
        .map_err(|_| ("invalid_revision", "settlement revision must fit 32 bits"))?;
    Ok((id, expected_revision))
}

fn settlement_player_arg(player_id: i64) -> Result<PlayerId, (&'static str, &'static str)> {
    u64::try_from(player_id)
        .ok()
        .and_then(|id| PlayerId::new(id).ok())
        .ok_or((
            "invalid_player_id",
            "player id must be a positive integer in the supported range",
        ))
}

fn settlement_role_arg(role: &str) -> Result<SettlementRole, (&'static str, String)> {
    let Some(role) = SettlementRole::ALL
        .into_iter()
        .find(|candidate| candidate.as_str().eq_ignore_ascii_case(role))
    else {
        return Err((
            "settlement.role_invalid",
            format!("unknown settlement role `{role}`"),
        ));
    };
    if role == SettlementRole::Owner {
        return Err((
            "settlement.role_invalid",
            "the owner role cannot be invited or assigned".to_string(),
        ));
    }
    Ok(role)
}

/// The bridge has no player registry: local mutations act as the current owner.
fn settlement_owner_actor(
    store: &SettlementStore,
    id: u64,
) -> Result<PlayerId, (&'static str, String)> {
    let Some(settlement) = store.get(id) else {
        let error = SettlementError::Unknown { id };
        return Err((error.code(), error.to_string()));
    };
    settlement.owner_id.ok_or_else(|| {
        let error = SettlementError::Unowned { id };
        (error.code(), error.to_string())
    })
}

fn settlement_mutation_payload(
    store: &SettlementStore,
    outcome: Result<Settlement, (&'static str, String)>,
) -> VarDictionary {
    match outcome {
        Ok(settlement) => {
            let mut result = VarDictionary::new();
            result.set("ok", true);
            result.set(
                "settlement",
                &settlement_to_dictionary(&settlement).to_variant(),
            );
            result.set(
                "checksum",
                &GString::from(&format!("{:016x}", store.checksum())),
            );
            result
        }
        Err((code, message)) => placement_simple_error(code, message),
    }
}

fn settlement_claim_in_store(
    store: &mut SettlementStore,
    id: u64,
    player: PlayerId,
    expected_revision: u32,
) -> Result<Settlement, (&'static str, String)> {
    store
        .claim(player, id, expected_revision)
        .map_err(|error| (error.code(), error.to_string()))
}

fn settlement_invite_in_store(
    store: &mut SettlementStore,
    id: u64,
    player: PlayerId,
    role: &str,
    expected_revision: u32,
) -> Result<Settlement, (&'static str, String)> {
    let role = settlement_role_arg(role)?;
    let actor = settlement_owner_actor(store, id)?;
    store
        .invite(actor, id, player, role, expected_revision)
        .map_err(|error| (error.code(), error.to_string()))
}

fn settlement_set_role_in_store(
    store: &mut SettlementStore,
    id: u64,
    player: PlayerId,
    role: &str,
    expected_revision: u32,
) -> Result<Settlement, (&'static str, String)> {
    let role = settlement_role_arg(role)?;
    let actor = settlement_owner_actor(store, id)?;
    store
        .set_member_role(actor, id, player, role, expected_revision)
        .map_err(|error| (error.code(), error.to_string()))
}

fn settlement_remove_member_in_store(
    store: &mut SettlementStore,
    id: u64,
    player: PlayerId,
    expected_revision: u32,
) -> Result<Settlement, (&'static str, String)> {
    let actor = settlement_owner_actor(store, id)?;
    store
        .remove_member(actor, id, player, expected_revision)
        .map_err(|error| (error.code(), error.to_string()))
}

fn settlement_transfer_in_store(
    store: &mut SettlementStore,
    id: u64,
    recipient: PlayerId,
    expected_revision: u32,
) -> Result<Settlement, (&'static str, String)> {
    let actor = settlement_owner_actor(store, id)?;
    store
        .transfer(actor, id, recipient, expected_revision)
        .map_err(|error| (error.code(), error.to_string()))
}

/// Clear ownership and every member without an owner actor (Admin override).
///
/// This is the bridge mirror of [`SettlementStore::force_release`]. The bridge
/// has no capability gate, so the developer editor is the only caller; the
/// revision guard still rejects a stale request.
fn settlement_release_in_store(
    store: &mut SettlementStore,
    id: u64,
    expected_revision: u32,
) -> Result<Settlement, (&'static str, String)> {
    store
        .force_release(id, expected_revision)
        .map_err(|error| (error.code(), error.to_string()))
}

fn settlement_set_tax_in_store(
    store: &mut SettlementStore,
    id: u64,
    rate: u16,
) -> Result<Settlement, (&'static str, String)> {
    let actor = settlement_owner_actor(store, id)?;
    store
        .set_tax_rate(actor, id, rate)
        .map_err(|error| (error.code(), error.to_string()))
}

/// FNV-1a feed shared by the bridge fingerprints.
fn fnv1a_push(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

/// Membership fingerprint for one settlement, as 16 lowercase hex digits.
///
/// FNV-1a over the settlement id, owner id, membership revision, and the
/// sorted member ids and role names. The store keeps its own whole-layer
/// checksum; this value lets a UI or smoke diff watch one record. It is a
/// display/diff value, not a security hash.
fn settlement_membership_checksum(settlement: &Settlement) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    fnv1a_push(&mut hash, &settlement.id.to_le_bytes());
    fnv1a_push(&mut hash, &settlement.membership_revision.to_le_bytes());
    match settlement.owner_id {
        Some(owner) => {
            fnv1a_push(&mut hash, &[1]);
            fnv1a_push(&mut hash, &owner.get().to_le_bytes());
        }
        None => fnv1a_push(&mut hash, &[0]),
    }
    let mut members = settlement.members.clone();
    members.sort_by_key(|member| member.player);
    for member in &members {
        fnv1a_push(&mut hash, &member.player.get().to_le_bytes());
        fnv1a_push(&mut hash, member.role.as_str().as_bytes());
        fnv1a_push(&mut hash, &[0]);
    }
    format!("{hash:016x}")
}

#[derive(serde::Serialize)]
struct SettlementMemberPayload {
    player: u64,
    role: &'static str,
    permissions: u8,
}

#[derive(serde::Serialize)]
struct SettlementPayload {
    id: u64,
    name: String,
    owner: String,
    owned: bool,
    owner_id: Option<u64>,
    center: [i32; 2],
    radius_m: u32,
    members: Vec<SettlementMemberPayload>,
    membership_revision: u32,
    membership_checksum: String,
    tax_rate: u16,
}

/// Pure settlement payload, testable without a Godot engine.
fn settlement_payload(settlement: &Settlement) -> SettlementPayload {
    let mut members = settlement.members.clone();
    members.sort_by_key(|member| member.player);
    SettlementPayload {
        id: settlement.id,
        name: settlement.name.clone(),
        owner: settlement.owner.clone(),
        owned: settlement.is_owned(),
        owner_id: settlement.owner_id.map(core_city::player::PlayerId::get),
        center: settlement.center,
        radius_m: settlement.radius_m,
        members: members
            .iter()
            .map(|member| SettlementMemberPayload {
                player: member.player.get(),
                role: member.role.as_str(),
                permissions: member.role.permissions().bits(),
            })
            .collect(),
        membership_revision: settlement.membership_revision,
        membership_checksum: settlement_membership_checksum(settlement),
        tax_rate: settlement.tax_rate,
    }
}

/// Read-only settlement payload. Every field is a pure function of the record.
fn settlement_to_dictionary(settlement: &Settlement) -> VarDictionary {
    let payload = settlement_payload(settlement);
    let mut result = VarDictionary::new();
    result.set("id", payload.id as i64);
    result.set("name", &GString::from(payload.name.as_str()));
    // `owner` stays the legacy display label; `owner_id` is authoritative.
    result.set("owner", &GString::from(payload.owner.as_str()));
    result.set("owned", payload.owned);
    let owner_id = match payload.owner_id {
        Some(owner) => Variant::from(owner as i64),
        None => Variant::nil(),
    };
    result.set("owner_id", &owner_id);
    result.set(
        "center",
        &PackedInt32Array::from(payload.center.to_vec()).to_variant(),
    );
    result.set("radius_m", i64::from(payload.radius_m));
    let mut payload_members: Array<VarDictionary> = Array::new();
    for member in &payload.members {
        let mut item = VarDictionary::new();
        item.set("player", member.player as i64);
        item.set("role", &GString::from(member.role));
        item.set("permissions", i64::from(member.permissions));
        payload_members.push(&item);
    }
    result.set("members", &payload_members.to_variant());
    result.set(
        "membership_revision",
        i64::from(payload.membership_revision),
    );
    result.set(
        "membership_checksum",
        &GString::from(payload.membership_checksum.as_str()),
    );
    result.set("tax_rate", i64::from(payload.tax_rate));
    result
}

/// Bridge-local player name limit in characters.
const BRIDGE_PLAYER_NAME_CHARS: usize = 32;

/// Player seed used while no world layer is open.
const BRIDGE_PLAYER_DEFAULT_SEED: u64 = 0;

/// Bridge-local player address book.
///
/// A newtype so `#[class(init)]` can default the field without a `Default`
/// impl on the engine-agnostic [`PlayerRegistry`].
struct LocalPlayers(PlayerRegistry);

impl Default for LocalPlayers {
    fn default() -> Self {
        Self(PlayerRegistry::new(BRIDGE_PLAYER_DEFAULT_SEED))
    }
}

/// Resolve the bridge player seed from the open world layers.
///
/// Empty layers are ignored. The first open seed wins, but every other open
/// layer must agree: a disagreement is reported instead of silently moving the
/// registry. [`BRIDGE_PLAYER_DEFAULT_SEED`] is used when nothing is open.
fn bridge_player_seed<I>(layers: I) -> Result<u64, (&'static str, &'static str)>
where
    I: IntoIterator<Item = Option<u64>>,
{
    let mut resolved: Option<u64> = None;
    for seed in layers.into_iter().flatten() {
        match resolved {
            None => resolved = Some(seed),
            Some(active) if active != seed => {
                return Err((
                    "player.seed_mismatch",
                    "open world layers disagree about the seed",
                ));
            }
            Some(_) => {}
        }
    }
    Ok(resolved.unwrap_or(BRIDGE_PLAYER_DEFAULT_SEED))
}

/// Reset `registry` when the world seed changes.
///
/// Returns `true` when the registry was replaced. A reset drops every name and
/// restarts ids at 1, so ids are stable only within one bridge seed.
fn player_registry_sync_seed(registry: &mut PlayerRegistry, seed: u64) -> bool {
    if registry.seed == seed {
        return false;
    }
    *registry = PlayerRegistry::new(seed);
    true
}

/// Validate and trim a bridge player name.
fn bridge_player_name(name: &str) -> Result<&str, (&'static str, &'static str)> {
    let name = name.trim();
    let invalid = name.is_empty()
        || name.chars().count() > BRIDGE_PLAYER_NAME_CHARS
        || name.chars().any(char::is_control);
    if invalid {
        return Err((
            "player.invalid_name",
            "player name must be 1-32 characters without control characters",
        ));
    }
    Ok(name)
}

/// Map a core player error to the bridge error vocabulary.
///
/// Name validation runs first, so a core name error should be unreachable; it
/// still maps to `player.invalid_name` rather than leaking `empty_name` or
/// `duplicate_name`. Id exhaustion is treated as the same hard limit as the
/// 256-player cap.
fn bridge_player_error(error: &PlayerError) -> (&'static str, String) {
    let code = match error {
        PlayerError::TooMany | PlayerError::IdExhausted => "player.limit",
        _ => "player.invalid_name",
    };
    (code, error.to_string())
}

/// Register a bridge player, returning the existing record for a duplicate.
///
/// Case-insensitive duplicates are idempotent. Names are trimmed and limited
/// to 32 characters here even though the core registry allows 48.
fn players_register_in_store(
    registry: &mut PlayerRegistry,
    name: &str,
) -> Result<Player, (&'static str, String)> {
    let name = bridge_player_name(name).map_err(|(code, message)| (code, message.to_string()))?;
    if let Some(existing) = registry.find_by_display_name(name).first() {
        return Ok((*existing).clone());
    }
    registry
        .register(name)
        .map_err(|error| bridge_player_error(&error))
}

/// One bridge player record as exposed to GDScript.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BridgePlayerRecord {
    id: u64,
    name: String,
}

/// Read-only player registry snapshot, testable without a Godot engine.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BridgePlayersSnapshot {
    count: usize,
    checksum: String,
    players: Vec<BridgePlayerRecord>,
}

fn bridge_player_record(player: &Player) -> BridgePlayerRecord {
    BridgePlayerRecord {
        id: player.id.get(),
        name: player.display_name.clone(),
    }
}

fn players_snapshot(registry: &PlayerRegistry) -> BridgePlayersSnapshot {
    BridgePlayersSnapshot {
        count: registry.count(),
        checksum: format!("{:016x}", registry.checksum()),
        players: registry
            .players()
            .iter()
            .map(bridge_player_record)
            .collect(),
    }
}

/// Find one player by display name; `None` when the name is unknown or blank.
fn players_find_record(registry: &PlayerRegistry, name: &str) -> Option<BridgePlayerRecord> {
    registry
        .find_by_display_name(name)
        .first()
        .map(|player| bridge_player_record(player))
}

fn bridge_player_dictionary(record: &BridgePlayerRecord) -> VarDictionary {
    let mut result = VarDictionary::new();
    result.set("id", record.id as i64);
    result.set("name", &GString::from(record.name.as_str()));
    result
}

fn players_register_payload(registry: &PlayerRegistry, player: &Player) -> VarDictionary {
    let snapshot = players_snapshot(registry);
    let record = bridge_player_record(player);
    let mut result = VarDictionary::new();
    result.set("ok", true);
    result.set("player", &bridge_player_dictionary(&record).to_variant());
    result.set("count", snapshot.count as i64);
    result.set("checksum", &GString::from(snapshot.checksum.as_str()));
    result
}

fn players_list_payload(registry: &PlayerRegistry) -> VarDictionary {
    let snapshot = players_snapshot(registry);
    let mut players: Array<VarDictionary> = Array::new();
    for record in &snapshot.players {
        players.push(&bridge_player_dictionary(record));
    }
    let mut result = VarDictionary::new();
    result.set("ok", true);
    result.set("count", snapshot.count as i64);
    result.set("checksum", &GString::from(snapshot.checksum.as_str()));
    result.set("players", &players.to_variant());
    result
}

fn players_find_payload(registry: &PlayerRegistry, name: &str) -> VarDictionary {
    let mut result = VarDictionary::new();
    result.set("ok", true);
    match players_find_record(registry, name) {
        Some(record) => {
            result.set("found", true);
            result.set("player", &bridge_player_dictionary(&record).to_variant());
        }
        None => {
            result.set("found", false);
            result.set("player", &Variant::nil());
        }
    }
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

/// Map unresolved `.tsbp` dependencies to the bridge error shape.
///
/// A package whose dependencies cannot be resolved is rejected before any
/// policy check, placement mutation, construction site, or treasury charge.
/// The payload carries the sorted missing IDs split by reason plus a stable
/// checksum, so a UI can show exactly what to install.
fn placement_dependency_error(
    path: &Path,
    error: core_blueprint::BlueprintDependencyError,
) -> VarDictionary {
    if error.issues.is_empty() {
        return placement_simple_error(
            "blueprint_load_failed",
            format!("{}: {}", path.display(), error.error),
        );
    }
    let mut missing = Vec::with_capacity(error.issues.len());
    let mut missing_mods = Vec::new();
    let mut missing_resource_packs = Vec::new();
    let mut checksum = 0xcbf2_9ce4_8422_2325_u64;
    for issue in &error.issues {
        let id = GString::from(issue.id.as_str());
        missing.push(id.clone());
        fnv1a_push(&mut checksum, issue.id.as_bytes());
        match issue.reason {
            DependencyReason::MissingMod => {
                fnv1a_push(&mut checksum, b"mod");
                missing_mods.push(id);
            }
            DependencyReason::MissingResourcePack => {
                fnv1a_push(&mut checksum, b"pack");
                missing_resource_packs.push(id);
            }
        }
        fnv1a_push(&mut checksum, &[0x1f]);
    }
    let names: Vec<&str> = error.issues.iter().map(|issue| issue.id.as_str()).collect();
    let mut result = placement_simple_error(
        "blueprint.missing_dependency",
        format!(
            "{} requires unavailable content: {}",
            path.display(),
            names.join(", ")
        ),
    );
    result.set(
        "missing",
        &PackedStringArray::from(missing.as_slice()).to_variant(),
    );
    result.set(
        "missing_mods",
        &PackedStringArray::from(missing_mods.as_slice()).to_variant(),
    );
    result.set(
        "missing_resource_packs",
        &PackedStringArray::from(missing_resource_packs.as_slice()).to_variant(),
    );
    result.set("dependency_checksum", format!("{checksum:016x}"));
    result
}

/// Map a D-034 content-identity failure to the usual bridge error shape.
fn content_identity_error_result(error: &ContentIdentityError) -> VarDictionary {
    placement_simple_error(error.code, error.message.as_str())
}

/// Reserved D-034 degraded-load refusal.
fn content_read_only_error() -> ContentIdentityError {
    ContentIdentityError {
        code: "content_read_only",
        message: "this world was opened without its required content pack; restore the pack and load it again before saving"
            .to_string(),
    }
}

/// Add the D-034 identity result fields shared by save and load.
fn set_content_identity_fields(
    result: &mut VarDictionary,
    status: &str,
    pack_id: &str,
    pack_checksum: &str,
    registry_checksum: &str,
    format_version: u32,
) {
    result.set("content_identity_status", status);
    result.set("content_pack_id", &GString::from(pack_id));
    result.set("content_pack_checksum", &GString::from(pack_checksum));
    result.set(
        "content_registry_checksum",
        &GString::from(registry_checksum),
    );
    result.set("content_format_version", i64::from(format_version));
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

/// Validate a GDScript blueprint library root.
fn blueprint_library_root_from_arg(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("blueprint library root is empty".to_string());
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err("blueprint library root must be absolute".to_string());
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err("blueprint library root must not contain '.' or '..'".to_string());
    }
    Ok(path)
}

/// Map a library failure to a stable bridge error code.
fn blueprint_library_error_code(error: &BlueprintLibraryError) -> &'static str {
    match error {
        BlueprintLibraryError::InvalidRoot { .. } => "invalid_blueprint_library_root",
        BlueprintLibraryError::RootMissing { .. } => "blueprint_library_root_missing",
        BlueprintLibraryError::RootNotDirectory { .. } => "blueprint_library_not_a_directory",
        BlueprintLibraryError::InvalidHash { .. } => "invalid_blueprint_library_hash",
        BlueprintLibraryError::InvalidPath { .. } => "invalid_blueprint_path",
        BlueprintLibraryError::MissingFile { .. } => "blueprint_library_missing_file",
        BlueprintLibraryError::NotAFile { .. } => "blueprint_library_not_a_file",
        BlueprintLibraryError::InvalidPackage { .. } => "blueprint_library_invalid_package",
        BlueprintLibraryError::TooLarge { .. } => "blueprint_library_too_large",
        BlueprintLibraryError::PathEscape { .. } => "blueprint_library_path_escape",
        BlueprintLibraryError::Collision { .. } => "blueprint_library_collision",
        BlueprintLibraryError::Io { source, .. }
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            "blueprint_library_missing_file"
        }
        BlueprintLibraryError::Io { .. } => "blueprint_library_io",
    }
}

pub(crate) fn blueprint_library_error_pair(
    error: &BlueprintLibraryError,
) -> (&'static str, String) {
    (blueprint_library_error_code(error), error.to_string())
}

pub(crate) fn blueprint_library_open(
    raw_root: &str,
) -> Result<BlueprintLibrary, (&'static str, String)> {
    let root = blueprint_library_root_from_arg(raw_root)
        .map_err(|message| ("invalid_blueprint_library_root", message))?;
    BlueprintLibrary::new(root).map_err(|error| blueprint_library_error_pair(&error))
}

fn blueprint_library_summary_from_arg(
    raw_root: &str,
) -> Result<LibrarySummary, (&'static str, String)> {
    blueprint_library_open(raw_root)?
        .summary()
        .map_err(|error| blueprint_library_error_pair(&error))
}

fn blueprint_library_insert_from_args(
    raw_root: &str,
    raw_package: &str,
) -> Result<(LibraryInsert, LibrarySummary), (&'static str, String)> {
    let library = blueprint_library_open(raw_root)?;
    let package = blueprint_path_from_arg(raw_package)
        .map_err(|message| ("invalid_blueprint_path", message))?;
    let insert = library
        .insert_file(&package)
        .map_err(|error| blueprint_library_error_pair(&error))?;
    let summary = library
        .summary()
        .map_err(|error| blueprint_library_error_pair(&error))?;
    Ok((insert, summary))
}

fn blueprint_library_resolve_from_args(
    raw_root: &str,
    hash: &str,
) -> Result<Option<PathBuf>, (&'static str, String)> {
    blueprint_library_open(raw_root)?
        .resolve_checked(hash)
        .map_err(|error| blueprint_library_error_pair(&error))
}

pub(crate) fn blueprint_library_summary_dictionary(summary: &LibrarySummary) -> VarDictionary {
    let mut result = VarDictionary::new();
    result.set("ok", true);
    result.set("count", summary.count as i64);
    result.set("total_bytes", summary.total_bytes as i64);
    result.set(
        "checksum",
        &GString::from(&format!("{:016x}", summary.checksum)),
    );
    result
}

/// Outcome of copying a world's unique packages into a blueprint library.
#[derive(Debug)]
struct WorldBlueprintSync {
    unique: usize,
    inserted: usize,
    existing: usize,
    library_count: u64,
}

/// Stamp every placement with its canonical content address.
///
/// Called on the saved clone only when a library is supplied. A plain save
/// never calls it, so legacy records keep their exact JSON and checksum.
fn stamp_world_blueprint_references(store: &mut PlacementStore) {
    for placement in &mut store.placements {
        placement.blueprint_ref = Some(format!("tsbp:{}", placement.blueprint_hash));
    }
}

/// Compare one loaded package with its placement record.
fn placement_package_error(
    package: &Blueprint,
    placement: &Placement,
) -> Option<(&'static str, String)> {
    if package.metadata.content_hash != placement.blueprint_hash {
        return Some((
            "blueprint_hash_mismatch",
            format!(
                "{} expected {} but the package is {}",
                placement.blueprint_path, placement.blueprint_hash, package.metadata.content_hash
            ),
        ));
    }
    let package_structure = structure_spec_for_blueprint(package);
    if package.metadata.id != placement.blueprint_id
        || package.metadata.name != placement.name
        || package.metadata.bounds_m != placement.bounds_m
        || package.metadata.stats.resident_capacity != placement.resident_capacity
        || package.metadata.stats.job_slots != placement.job_slots
        || placement
            .structure
            .is_some_and(|structure| package_structure != Some(structure))
    {
        return Some((
            "blueprint_metadata_mismatch",
            format!(
                "{} metadata does not match the placement record",
                placement.blueprint_path
            ),
        ));
    }
    None
}

fn world_save_package_error(path: &Path, error: &BlueprintError) -> (&'static str, String) {
    let code = match error {
        BlueprintError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
            "blueprint_library_missing_file"
        }
        BlueprintError::HashMismatch { .. } => "blueprint_hash_mismatch",
        _ => "blueprint_library_invalid_package",
    };
    (code, format!("{}: {error}", path.display()))
}

/// Copy every unique placement package into `library`, validating first.
///
/// All unique sources are loaded and checked against every placement record
/// before the first insert, so a missing, corrupt, or mismatched package leaves
/// the library unchanged. Repeated packages and repeat saves are no-ops.
///
/// A record whose recorded `blueprint_path` is missing or unreadable is
/// resolved by its `blueprint_ref` when present; the library path is used only
/// as a source and is never written back to the record.
fn sync_world_blueprints(
    store: &PlacementStore,
    library: &BlueprintLibrary,
) -> Result<WorldBlueprintSync, (&'static str, String)> {
    let summary = library
        .summary()
        .map_err(|error| blueprint_library_error_pair(&error))?;
    let mut hashes: BTreeSet<String> = BTreeSet::new();
    for placement in &store.placements {
        placement_reference_hash(placement)?;
        hashes.insert(placement.blueprint_hash.clone());
    }
    if hashes.is_empty() {
        return Ok(WorldBlueprintSync {
            unique: 0,
            inserted: 0,
            existing: 0,
            library_count: summary.count,
        });
    }

    let mut packages: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut loaded: BTreeMap<String, Blueprint> = BTreeMap::new();
    for hash in &hashes {
        let recorded = store
            .placements
            .iter()
            .find(|placement| placement.blueprint_hash == *hash)
            .expect("every hash has a placement");
        let recorded_path = blueprint_path_from_arg(&recorded.blueprint_path);
        let recorded_result = match &recorded_path {
            Ok(path) => load_tsbp(path).map_err(|error| world_save_package_error(path, &error)),
            Err(error) => Err((
                "invalid_blueprint_path",
                format!("{}: {error}", recorded.blueprint_path),
            )),
        };
        let (source, package) = match recorded_result {
            Ok(package) => (recorded_path.expect("loaded path parsed"), package),
            Err(recorded_error) => {
                let Some(reference) = store
                    .placements
                    .iter()
                    .find(|placement| {
                        placement.blueprint_hash == *hash && placement.blueprint_ref.is_some()
                    })
                    .and_then(|placement| placement.blueprint_ref.as_deref())
                else {
                    return Err(recorded_error);
                };
                let expected = format!("tsbp:{hash}");
                if reference != expected {
                    return Err((
                        "blueprint_hash_mismatch",
                        format!(
                            "{} records reference {reference} for hash {hash}",
                            recorded.blueprint_path
                        ),
                    ));
                }
                let path = library
                    .resolve_checked(hash)
                    .map_err(|error| blueprint_library_error_pair(&error))?
                    .ok_or_else(|| {
                        (
                            "blueprint_library_missing_file",
                            format!(
                                "{hash} is not in the library at {}",
                                library.root().display()
                            ),
                        )
                    })?;
                let package = load_tsbp(&path).map_err(|error| {
                    (
                        "blueprint_library_invalid_package",
                        format!("{}: {error}", path.display()),
                    )
                })?;
                (path, package)
            }
        };
        if package.metadata.content_hash.as_str() != hash.as_str() {
            return Err((
                "blueprint_hash_mismatch",
                format!(
                    "{} expected {hash} but the package is {}",
                    source.display(),
                    package.metadata.content_hash
                ),
            ));
        }
        packages.insert(hash.clone(), source);
        loaded.insert(hash.clone(), package);
    }
    for placement in &store.placements {
        let package = loaded
            .get(placement.blueprint_hash.as_str())
            .expect("every placement package was loaded");
        if let Some((code, message)) = placement_package_error(package, placement) {
            return Err((code, message));
        }
    }
    let mut inserted = 0_usize;
    let mut existing = 0_usize;
    for (hash, path) in &packages {
        let insert = library
            .insert_file(path)
            .map_err(|error| blueprint_library_error_pair(&error))?;
        if insert.hash.as_str() != hash.as_str() {
            return Err((
                "blueprint_hash_mismatch",
                format!(
                    "{} resolved to {} but the save records {hash}",
                    path.display(),
                    insert.hash
                ),
            ));
        }
        if insert.inserted {
            inserted += 1;
        } else {
            existing += 1;
        }
    }
    let summary = library
        .summary()
        .map_err(|error| blueprint_library_error_pair(&error))?;
    Ok(WorldBlueprintSync {
        unique: packages.len(),
        inserted,
        existing,
        library_count: summary.count,
    })
}

/// Parse and validate a placement's optional content-addressed reference.
///
/// The only accepted shape is `tsbp:` followed by the record's canonical
/// `blake3:<64 lowercase hex>` hash. A different well-formed hash is a
/// mismatch; any other shape is a malformed reference.
fn placement_reference_hash(placement: &Placement) -> Result<Option<&str>, (&'static str, String)> {
    let Some(reference) = placement.blueprint_ref.as_deref() else {
        return Ok(None);
    };
    let Some(hash) = reference.strip_prefix("tsbp:") else {
        return Err((
            "invalid_blueprint_reference",
            format!(
                "{}: reference must start with tsbp:",
                placement.blueprint_path
            ),
        ));
    };
    if BlueprintLibrary::validate_hash(hash).is_err() {
        return Err((
            "invalid_blueprint_reference",
            format!(
                "{}: reference {reference} is not tsbp:blake3:<64 hex>",
                placement.blueprint_path
            ),
        ));
    }
    if hash != placement.blueprint_hash {
        return Err((
            "blueprint_hash_mismatch",
            format!(
                "{} reference {reference} does not match recorded hash {}",
                placement.blueprint_path, placement.blueprint_hash
            ),
        ));
    }
    Ok(Some(hash))
}

/// Which lookup path produced a placement package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlacementPackageSource {
    /// The legacy recorded path was used.
    Recorded,
    /// The content-addressed reference resolved in the library.
    Reference,
    /// A legacy record without a reference fell back to the library hash.
    LibraryHash,
    /// A record with a reference loaded by its path because no library was
    /// configured.
    LegacyPathFallback,
}

/// Load one placement package through the reference-first resolution order.
///
/// The second tuple item is the library path only when the library package was
/// used. A reference is never treated as a path: without a library, the
/// recorded path is tried and a failed lookup reports
/// `blueprint_reference_requires_library`. A reference that resolves to an
/// invalid or mismatched package fails closed.
fn load_world_placement_package(
    placement: &Placement,
    library: Option<&BlueprintLibrary>,
) -> Result<(Blueprint, Option<PathBuf>, PlacementPackageSource), (&'static str, String)> {
    let reference = placement_reference_hash(placement)?;
    if let Some(hash) = reference {
        let Some(library) = library else {
            return load_recorded_world_package(placement).map_or_else(
                |recorded_error| {
                    Err((
                        "blueprint_reference_requires_library",
                        format!(
                            "{}: blueprint reference {hash} requires a blueprint library; recorded package failed: {}",
                            placement.blueprint_path, recorded_error.1
                        ),
                    ))
                },
                |package| Ok((package, None, PlacementPackageSource::LegacyPathFallback)),
            );
        };
        let path = library
            .resolve_checked(hash)
            .map_err(|error| blueprint_library_error_pair(&error))?
            .ok_or_else(|| {
                (
                    "blueprint_library_missing_file",
                    format!(
                        "{hash} is not in the library at {}",
                        library.root().display()
                    ),
                )
            })?;
        let package = load_tsbp(&path).map_err(|error| {
            (
                "blueprint_library_invalid_package",
                format!("{}: {error}", path.display()),
            )
        })?;
        if let Some((code, message)) = placement_package_error(&package, placement) {
            return Err((code, message));
        }
        return Ok((package, Some(path), PlacementPackageSource::Reference));
    }

    match load_recorded_world_package(placement) {
        Ok(package) => Ok((package, None, PlacementPackageSource::Recorded)),
        Err(recorded_error) => {
            let Some(library) = library else {
                return Err(recorded_error);
            };
            let resolved = library
                .resolve_checked(&placement.blueprint_hash)
                .map_err(|error| blueprint_library_error_pair(&error))?;
            let Some(path) = resolved else {
                return Err((
                    "blueprint_library_missing_file",
                    format!(
                        "{}: blueprint {} is not in the library at {}",
                        placement.blueprint_path,
                        placement.blueprint_hash,
                        library.root().display()
                    ),
                ));
            };
            let package = load_tsbp(&path).map_err(|error| {
                (
                    "blueprint_library_invalid_package",
                    format!("{}: {error}", path.display()),
                )
            })?;
            if let Some((code, message)) = placement_package_error(&package, placement) {
                return Err((code, message));
            }
            Ok((package, Some(path), PlacementPackageSource::LibraryHash))
        }
    }
}

/// Load a placement package from its recorded absolute path.
fn load_recorded_world_package(placement: &Placement) -> Result<Blueprint, (&'static str, String)> {
    let path = blueprint_path_from_arg(&placement.blueprint_path).map_err(|error| {
        (
            "invalid_blueprint_path",
            format!("{}: {error}", placement.blueprint_path),
        )
    })?;
    let package = load_tsbp(&path).map_err(|error| {
        (
            blueprint_error_code(&error),
            format!("{}: {error}", placement.blueprint_path),
        )
    })?;
    if let Some((code, message)) = placement_package_error(&package, placement) {
        return Err((code, message));
    }
    Ok(package)
}

/// Loaded package facts for the placements in a world save.
#[derive(Debug)]
struct LoadedWorldPlacements {
    solid_by_id: BTreeMap<u64, u64>,
    resolved_paths: BTreeMap<String, String>,
    resolved_references: usize,
    legacy_path_fallbacks: usize,
}

/// Load every placement package and return the recorded solid voxel counts
/// plus the content-hash keyed library paths used as fallbacks.
fn load_world_placement_packages(
    store: &PlacementStore,
    library: Option<&BlueprintLibrary>,
) -> Result<LoadedWorldPlacements, (&'static str, String)> {
    let mut solid_by_id = BTreeMap::new();
    let mut resolved_paths = BTreeMap::new();
    let mut resolved_references = 0;
    let mut legacy_path_fallbacks = 0;
    for placement in &store.placements {
        let (package, resolved, source) = load_world_placement_package(placement, library)?;
        match source {
            PlacementPackageSource::Reference => resolved_references += 1,
            PlacementPackageSource::LegacyPathFallback => legacy_path_fallbacks += 1,
            PlacementPackageSource::Recorded | PlacementPackageSource::LibraryHash => {}
        }
        solid_by_id.insert(placement.id, package.metadata.solid_voxels);
        if let Some(path) = resolved {
            resolved_paths
                .entry(placement.blueprint_hash.clone())
                .or_insert_with(|| path.to_string_lossy().into_owned());
        }
    }
    Ok(LoadedWorldPlacements {
        solid_by_id,
        resolved_paths,
        resolved_references,
        legacy_path_fallbacks,
    })
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
    if let Some(blueprint_ref) = placement.blueprint_ref.as_deref() {
        result.set("blueprint_ref", &GString::from(blueprint_ref));
    }
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

/// Placement dictionary plus the bridge-only library resolution, when any.
///
/// `resolved` is the content-hash keyed path from the last portable load. It
/// is attached only when it differs from the recorded path; the recorded
/// `blueprint_path` stays authoritative for the save format.
fn placement_to_dictionary_with_resolved(
    placement: &Placement,
    resolved: Option<&str>,
) -> VarDictionary {
    let mut result = placement_to_dictionary(placement);
    if let Some(path) = resolved {
        result.set("resolved_blueprint_path", &GString::from(path));
    }
    result
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
    active_content_ids: &[&str],
) -> Result<(u64, PlacementRequest), VarDictionary> {
    let seed = u64::try_from(seed)
        .map_err(|_| placement_simple_error("invalid_seed", "world seed must be non-negative"))?;
    let rotation = normalize_rotation(rotation).map_err(|error| placement_error_result(&error))?;
    let path = blueprint_path_from_arg(path)
        .map_err(|error| placement_simple_error("invalid_blueprint_path", error))?;
    let blueprint = load_tsbp_with_dependency_issues(&path, active_content_ids)
        .map_err(|error| placement_dependency_error(&path, error))?;
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

/// Weighted resident and business tax multipliers for one treasury tick.
///
/// The resident side averages the district tax multiplier resolved at every
/// completed home with residents, weighted by those residents. The business
/// side does the same at every completed workplace with staff, weighted by
/// staffed jobs. This is the aggregate model's majority-workplace
/// approximation: individual workers are not materialized, so each workplace's
/// staff count is weighted at that workplace's own centre.
///
/// A point outside every policy contributes the neutral multiplier `1.0`, and
/// overlapping policies resolve through `effect_at`, so the smallest policy id
/// wins. Missing, empty, or foreign-seed registries, foreign placement or
/// population seeds, and a non-finite weighted total all return
/// [`PolicyTaxWeights::ONE`]. A side with no residents or staff stays neutral
/// while the other side keeps its weight, and the final pair is validated by
/// [`PolicyTaxWeights::new`]; anything out of range falls back to neutral.
fn policy_tax_weights(
    policies: Option<&DistrictPolicyStore>,
    seed: u64,
    placement_seed: Option<u64>,
    population: Option<&CityRuntime>,
    homes: &BTreeMap<u64, [f32; 2]>,
    workplaces: &[Workplace],
    staffing: &BTreeMap<u64, u32>,
) -> PolicyTaxWeights {
    let Some(policies) = policies else {
        return PolicyTaxWeights::ONE;
    };
    if policies.seed != seed || policies.is_empty() {
        return PolicyTaxWeights::ONE;
    }
    if placement_seed.is_some_and(|placement| placement != seed) {
        return PolicyTaxWeights::ONE;
    }
    let Some(population) = population else {
        return PolicyTaxWeights::ONE;
    };
    if population.seed != seed {
        return PolicyTaxWeights::ONE;
    }
    let resident = weighted_tax_multiplier(
        policies,
        homes.iter().filter_map(|(&id, center)| {
            let residents = population
                .households
                .get(&id)
                .map_or(0, |household| household.residents);
            (residents > 0).then_some((*center, f64::from(residents)))
        }),
    )
    .unwrap_or(1.0);
    let business = weighted_tax_multiplier(
        policies,
        workplaces.iter().map(|workplace| {
            let staffed = staffing.get(&workplace.id).copied().unwrap_or(0);
            (workplace.center, f64::from(staffed))
        }),
    )
    .unwrap_or(1.0);
    PolicyTaxWeights::new(resident, business).unwrap_or(PolicyTaxWeights::ONE)
}

/// Weighted average of the district tax multiplier across `contributions`.
///
/// `None` means the contribution weights are empty or produced a non-finite
/// average, so the caller keeps that tax side neutral.
fn weighted_tax_multiplier(
    policies: &DistrictPolicyStore,
    contributions: impl Iterator<Item = ([f32; 2], f64)>,
) -> Option<f64> {
    let mut weighted = 0.0;
    let mut weight = 0.0;
    for (center, contribution) in contributions {
        if !contribution.is_finite() || contribution <= 0.0 {
            continue;
        }
        let multiplier = policies
            .effect_at(center, 1.0, 1.0)
            .map_or(1.0, |effect| f64::from(effect.tax_multiplier));
        weighted += multiplier * contribution;
        weight += contribution;
    }
    if !weight.is_finite() || weight <= 0.0 || !weighted.is_finite() {
        return None;
    }
    let average = weighted / weight;
    average.is_finite().then_some(average)
}

/// Weighted service-funding multiplier for one treasury tick.
///
/// Every **completed** civic service site contributes its district
/// `service_funding` value, resolved by [`DistrictPolicyStore::policy_at`] at
/// the site centre, and the mean is `sum(funding) / site_count`. A site outside
/// every circle contributes the neutral multiplier `1.0`, and overlapping
/// policies resolve through the smallest policy id.
///
/// Missing, empty, or foreign-seed registries, foreign placement, population,
/// or service seeds, and a non-finite multiplier return
/// [`PolicyServiceWeight::ONE`]. A city with no completed service sites also
/// stays neutral, so utility-only infrastructure is unaffected.
fn policy_service_funding_weight(
    policies: Option<&DistrictPolicyStore>,
    seed: u64,
    placement_seed: Option<u64>,
    population_seed: Option<u64>,
    services: Option<&ServiceStore>,
) -> PolicyServiceWeight {
    let Some(policies) = policies else {
        return PolicyServiceWeight::ONE;
    };
    if policies.seed != seed || policies.is_empty() {
        return PolicyServiceWeight::ONE;
    }
    if placement_seed.is_some_and(|placement| placement != seed) {
        return PolicyServiceWeight::ONE;
    }
    if population_seed.is_some_and(|population| population != seed) {
        return PolicyServiceWeight::ONE;
    }
    let Some(services) = services else {
        return PolicyServiceWeight::ONE;
    };
    if services.seed != seed {
        return PolicyServiceWeight::ONE;
    }
    let mut total = 0.0;
    let mut sites = 0_u64;
    for site in services.sites.values().filter(|site| site.is_operational()) {
        let funding = policies
            .policy_at([site.center[0], site.center[2]])
            .map_or(1.0, |policy| f64::from(policy.service_funding));
        if !funding.is_finite() {
            return PolicyServiceWeight::ONE;
        }
        total += funding;
        sites += 1;
    }
    if sites == 0 || !total.is_finite() {
        return PolicyServiceWeight::ONE;
    }
    PolicyServiceWeight::new(total / sites as f64).unwrap_or(PolicyServiceWeight::ONE)
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
///
/// The envelope must be a JSON object. Checking it before assignment keeps a
/// non-object document from panicking at the indexing operation and reports
/// `invalid_json`; a serialization failure is `save_failed`.
fn attach_policies(
    mut value: serde_json::Value,
    store: &DistrictPolicyStore,
) -> Result<serde_json::Value, (&'static str, String)> {
    if !value.is_object() {
        return Err((
            "invalid_json",
            "the world save is not a JSON object".to_string(),
        ));
    }
    value["policies"] =
        serde_json::to_value(store).map_err(|error| ("save_failed", error.to_string()))?;
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
    let value = attach_policies(value, policies)?;
    serde_json::to_string_pretty(&value).map_err(|error| ("save_failed", error.to_string()))
}

/// Read the optional `policies` object from a world save.
///
/// Old saves have no object and load with the registry closed. A malformed
/// document, a non-object envelope, a malformed object, an invalid registry,
/// or a registry for another world seed is rejected so a failed load never
/// installs a partial store.
fn policies_from_world_json(
    json: &str,
    expected_seed: u64,
) -> Result<Option<DistrictPolicyStore>, (&'static str, String)> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| ("invalid_json", error.to_string()))?;
    if !value.is_object() {
        return Err((
            "invalid_json",
            "the world save is not a JSON object".to_string(),
        ));
    }
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

/// One placement denial resolved from the live district-policy registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PlacementPolicyDenialPayload {
    /// Id of the smallest containing policy that produced the denial.
    policy_id: u64,
    /// Which rule denied the blueprint.
    kind: PolicyPlacementDenial,
}

/// Resolve the placement rule for one point and blueprint.
///
/// Returns `None` when no registry is open, the registry belongs to another
/// seed, or no enforceable policy contains the point. Overlaps resolve through
/// [`DistrictPolicyStore::policy_at`], so the smallest containing id decides
/// even when a larger id would permit the blueprint. A non-enforceable record
/// is left alone and permits the placement.
fn placement_policy_denial(
    policies: Option<&DistrictPolicyStore>,
    seed: u64,
    center: [f32; 2],
    blueprint_id: &str,
) -> Option<PlacementPolicyDenialPayload> {
    let store = policies?;
    if store.seed != seed {
        return None;
    }
    let policy = store.policy_at(center)?;
    let kind = policy_placement_denial(policy, blueprint_id)?;
    Some(PlacementPolicyDenialPayload {
        policy_id: policy.id,
        kind,
    })
}

/// Resolve the district-policy denial for one placement request.
///
/// [`TownsmithCore::world_place_blueprint`] and
/// [`TownsmithCore::world_validate_placement`] both call this after
/// [`world_placement_request`], so a ghost preview and the commit path report
/// the same stable code and message. The check is read-only: it borrows the
/// optional registry and never mutates a store.
fn placement_policy_failure(
    policies: Option<&DistrictPolicyStore>,
    seed: u64,
    center: [f32; 2],
    blueprint_id: &str,
) -> Option<(&'static str, String)> {
    let denial = placement_policy_denial(policies, seed, center, blueprint_id)?;
    Some((
        denial.kind.code(),
        format!(
            "district policy {} blocks placing blueprint {} here",
            denial.policy_id, blueprint_id
        ),
    ))
}

/// Bridge error dictionary for one placement policy denial.
fn placement_policy_error(
    policies: Option<&DistrictPolicyStore>,
    seed: u64,
    center: [f32; 2],
    blueprint_id: &str,
) -> Option<VarDictionary> {
    let (code, message) = placement_policy_failure(policies, seed, center, blueprint_id)?;
    Some(placement_simple_error(code, message))
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
mod content_identity_faults;

#[cfg(test)]
mod save_compat_tests;

#[cfg(test)]
mod save_composition_matrix;

#[cfg(test)]
mod player_registry_faults;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
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
    fn placement_policy_denial_is_neutral_without_a_store_or_policy() {
        assert_eq!(
            placement_policy_denial(None, 2026, [100.0, 100.0], "townsmith:hut"),
            None
        );
        let empty = DistrictPolicyStore::new(2026);
        assert_eq!(
            placement_policy_denial(Some(&empty), 2026, [100.0, 100.0], "townsmith:hut"),
            None
        );
        let mut store = DistrictPolicyStore::new(2026);
        store
            .create(&district_policy_request("Riverside"))
            .expect("create");
        assert_eq!(
            placement_policy_denial(Some(&store), 2026, [100.0, 100.0], "townsmith:hut"),
            None
        );
        assert_eq!(
            placement_policy_denial(Some(&store), 2026, [500.0, 500.0], "townsmith:hut"),
            None
        );
    }

    #[test]
    fn placement_policy_denial_reports_construction_disabled() {
        let mut store = DistrictPolicyStore::new(2026);
        let mut request = district_policy_request("Riverside");
        request.construction_allowed = false;
        let policy = store.create(&request).expect("create");
        assert_eq!(
            placement_policy_denial(Some(&store), 2026, [100.0, 100.0], "townsmith:hut"),
            Some(PlacementPolicyDenialPayload {
                policy_id: policy.id,
                kind: PolicyPlacementDenial::ConstructionDisabled,
            })
        );
        assert_eq!(
            PolicyPlacementDenial::ConstructionDisabled.code(),
            "policy_construction_disabled"
        );
    }

    #[test]
    fn placement_policy_denial_reports_an_allowlist_miss() {
        let mut store = DistrictPolicyStore::new(2026);
        let mut request = district_policy_request("Old Town");
        request.blueprint_allowlist =
            std::collections::BTreeSet::from(["townsmith:shop".to_string()]);
        let policy = store.create(&request).expect("create");
        assert_eq!(
            placement_policy_denial(Some(&store), 2026, [100.0, 100.0], "townsmith:hut"),
            Some(PlacementPolicyDenialPayload {
                policy_id: policy.id,
                kind: PolicyPlacementDenial::BlueprintNotAllowed,
            })
        );
        assert_eq!(
            placement_policy_denial(Some(&store), 2026, [100.0, 100.0], "townsmith:shop"),
            None
        );
        assert_eq!(
            PolicyPlacementDenial::BlueprintNotAllowed.code(),
            "policy_blueprint_not_allowed"
        );
    }

    #[test]
    fn placement_policy_denial_prefers_the_smallest_overlapping_id() {
        let mut store = DistrictPolicyStore::new(2026);
        let mut denying = district_policy_request("Riverside");
        denying.construction_allowed = false;
        let first = store.create(&denying).expect("first");
        let second = store
            .create(&district_policy_request("Old Town"))
            .expect("second");
        assert!(first.id < second.id);

        assert_eq!(
            placement_policy_denial(Some(&store), 2026, [100.0, 100.0], "townsmith:hut"),
            Some(PlacementPolicyDenialPayload {
                policy_id: first.id,
                kind: PolicyPlacementDenial::ConstructionDisabled,
            }),
            "the larger permissive policy must not override the smaller id"
        );

        let mut allowing_first = DistrictPolicyStore::new(2026);
        let mut restrict_second = district_policy_request("Old Town");
        restrict_second.construction_allowed = false;
        let first = allowing_first
            .create(&district_policy_request("Riverside"))
            .expect("first");
        let second = allowing_first.create(&restrict_second).expect("second");
        assert!(first.id < second.id);
        assert_eq!(
            placement_policy_denial(Some(&allowing_first), 2026, [100.0, 100.0], "townsmith:hut"),
            None,
            "the smaller permissive policy decides the overlap"
        );
    }

    #[test]
    fn placement_policy_denial_ignores_a_foreign_seed() {
        let mut store = DistrictPolicyStore::new(2026);
        let mut request = district_policy_request("Riverside");
        request.construction_allowed = false;
        store.create(&request).expect("create");
        assert_eq!(
            placement_policy_denial(Some(&store), 2027, [100.0, 100.0], "townsmith:hut"),
            None
        );
    }

    #[test]
    fn placement_policy_denial_leaves_placements_and_economy_untouched() {
        let mut store = DistrictPolicyStore::new(2026);
        let mut request = district_policy_request("Riverside");
        request.construction_allowed = false;
        store.create(&request).expect("create");

        let placements = PlacementStore::new(2026);
        let placement_checksum = placements.checksum();
        let economy = Economy::new(2026);
        let balance = economy.balance;

        let denial = placement_policy_denial(Some(&store), 2026, [100.0, 100.0], "townsmith:hut")
            .expect("construction is disabled");
        assert_eq!(denial.kind, PolicyPlacementDenial::ConstructionDisabled);
        assert_eq!(placements.checksum(), placement_checksum);
        assert_eq!(placements.placements.len(), 0);
        assert_eq!(economy.balance, balance);
    }

    #[test]
    fn placement_policy_failure_denies_disabled_construction_on_validation() {
        let mut store = DistrictPolicyStore::new(2026);
        let mut request = district_policy_request("Riverside");
        request.construction_allowed = false;
        let policy = store.create(&request).expect("create");

        let (code, message) =
            placement_policy_failure(Some(&store), 2026, [100.0, 100.0], "townsmith:hut")
                .expect("the preview must be denied");
        assert_eq!(code, "policy_construction_disabled");
        assert_eq!(
            message,
            format!(
                "district policy {} blocks placing blueprint townsmith:hut here",
                policy.id
            )
        );
    }

    #[test]
    fn placement_policy_failure_denies_an_allowlist_miss_on_validation() {
        let mut store = DistrictPolicyStore::new(2026);
        let mut request = district_policy_request("Old Town");
        request.blueprint_allowlist =
            std::collections::BTreeSet::from(["townsmith:shop".to_string()]);
        let policy = store.create(&request).expect("create");

        let (code, message) =
            placement_policy_failure(Some(&store), 2026, [100.0, 100.0], "townsmith:hut")
                .expect("the preview must be denied");
        assert_eq!(code, "policy_blueprint_not_allowed");
        assert_eq!(
            message,
            format!(
                "district policy {} blocks placing blueprint townsmith:hut here",
                policy.id
            )
        );

        // The exact allowlisted id still previews.
        assert_eq!(
            placement_policy_failure(Some(&store), 2026, [100.0, 100.0], "townsmith:shop"),
            None
        );
    }

    #[test]
    fn placement_policy_failure_permits_an_allowlisted_blueprint_on_validation() {
        let mut store = DistrictPolicyStore::new(2026);
        let mut request = district_policy_request("Riverside");
        request.blueprint_allowlist =
            std::collections::BTreeSet::from(["townsmith:hut".to_string()]);
        store.create(&request).expect("create");

        assert_eq!(
            placement_policy_failure(Some(&store), 2026, [100.0, 100.0], "townsmith:hut"),
            None
        );
        // An empty allowlist is unrestricted.
        let mut unrestricted = DistrictPolicyStore::new(2026);
        unrestricted
            .create(&district_policy_request("Unrestricted"))
            .expect("create");
        assert_eq!(
            placement_policy_failure(Some(&unrestricted), 2026, [100.0, 100.0], "townsmith:hut"),
            None
        );
    }

    #[test]
    fn placement_policy_failure_is_neutral_without_a_store_or_a_containing_policy() {
        // No registry.
        assert_eq!(
            placement_policy_failure(None, 2026, [100.0, 100.0], "townsmith:hut"),
            None
        );
        // Open but empty registry.
        let empty = DistrictPolicyStore::new(2026);
        assert_eq!(
            placement_policy_failure(Some(&empty), 2026, [100.0, 100.0], "townsmith:hut"),
            None
        );

        let mut store = DistrictPolicyStore::new(2026);
        let mut request = district_policy_request("Riverside");
        request.construction_allowed = false;
        store.create(&request).expect("create");

        // Outside every circle.
        assert_eq!(
            placement_policy_failure(Some(&store), 2026, [500.0, 500.0], "townsmith:hut"),
            None
        );
        // Foreign seed.
        assert_eq!(
            placement_policy_failure(Some(&store), 2027, [100.0, 100.0], "townsmith:hut"),
            None
        );
    }

    #[test]
    fn placement_policy_failure_prefers_the_smallest_overlapping_id() {
        let mut denying_first = DistrictPolicyStore::new(2026);
        let mut denying = district_policy_request("Riverside");
        denying.construction_allowed = false;
        let first = denying_first.create(&denying).expect("first");
        let second = denying_first
            .create(&district_policy_request("Old Town"))
            .expect("second");
        assert!(first.id < second.id);

        let (code, message) =
            placement_policy_failure(Some(&denying_first), 2026, [100.0, 100.0], "townsmith:hut")
                .expect("the smaller denying policy wins");
        assert_eq!(code, "policy_construction_disabled");
        assert_eq!(
            message,
            format!(
                "district policy {} blocks placing blueprint townsmith:hut here",
                first.id
            )
        );

        let mut allowing_first = DistrictPolicyStore::new(2026);
        let first = allowing_first
            .create(&district_policy_request("Riverside"))
            .expect("first");
        let mut restrict_second = district_policy_request("Old Town");
        restrict_second.construction_allowed = false;
        let second = allowing_first.create(&restrict_second).expect("second");
        assert!(first.id < second.id);
        assert_eq!(
            placement_policy_failure(Some(&allowing_first), 2026, [100.0, 100.0], "townsmith:hut"),
            None,
            "the smaller permissive policy decides the overlap"
        );
    }

    #[test]
    fn denied_validation_leaves_placements_economy_and_checksum_untouched() {
        let mut store = DistrictPolicyStore::new(2026);
        let mut request = district_policy_request("Riverside");
        request.construction_allowed = false;
        store.create(&request).expect("create");

        let placements = PlacementStore::new(2026);
        let placement_checksum = placements.checksum();
        let economy = Economy::new(2026);
        let balance = economy.balance;

        let (code, _) =
            placement_policy_failure(Some(&store), 2026, [100.0, 100.0], "townsmith:hut")
                .expect("the preview must be denied");
        assert_eq!(code, "policy_construction_disabled");

        // Validation stays read-only: no placement, charge, or checksum move.
        assert_eq!(placements.checksum(), placement_checksum);
        assert_eq!(placements.checksum(), PlacementStore::new(2026).checksum());
        assert_eq!(placements.placements.len(), 0);
        assert_eq!(economy.balance, balance);
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
    fn district_policy_world_save_round_trips_the_registry() {
        let mut store = DistrictPolicyStore::new(2026);
        let first = store
            .create(&district_policy_request("Riverside"))
            .expect("first");
        let mut changed = district_policy_request("Old Town");
        changed.construction_allowed = false;
        changed
            .blueprint_allowlist
            .insert("base:workshop".to_string());
        let second = store.create(&changed).expect("second");
        let base = "{\n  \"format_version\": 1,\n  \"seed\": 2026,\n  \"next_id\": 1,\n  \"placements\": []\n}\n";

        let json = attach_optional_policies(base.to_string(), Some(&store), 2026).expect("attach");
        assert_ne!(json.as_bytes(), base.as_bytes());
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("save JSON");
        assert_eq!(parsed["seed"], 2026);
        assert_eq!(parsed["next_id"], 1);
        assert_eq!(parsed["policies"]["seed"], 2026);
        assert_eq!(
            parsed["policies"]["policies"]
                .as_object()
                .map(|map| map.len()),
            Some(2)
        );

        let loaded = policies_from_world_json(&json, 2026)
            .expect("load")
            .expect("registry");
        assert_eq!(loaded, store);
        assert_eq!(loaded.get(first.id), Some(&first));
        assert_eq!(loaded.get(second.id), Some(&second));
        assert_eq!(loaded.checksum(), store.checksum());
        assert_eq!(loaded.next_id, 3);
    }

    #[test]
    fn district_policy_world_load_accepts_an_old_save_without_policies() {
        let base = "{\n  \"format_version\": 1,\n  \"seed\": 2026,\n  \"next_id\": 1,\n  \"placements\": []\n}\n";
        assert_eq!(policies_from_world_json(base, 2026), Ok(None));
    }

    #[test]
    fn district_policy_world_save_keeps_json_byte_identical_without_a_registry() {
        let base = "{\n  \"format_version\": 1,\n  \"seed\": 2026,\n  \"next_id\": 1,\n  \"placements\": []\n}\n";
        let saved = attach_optional_policies(base.to_string(), None, 2026).expect("save");
        assert_eq!(saved.as_bytes(), base.as_bytes());
        assert!(!saved.contains("policies"));
    }

    #[test]
    fn district_policy_world_save_and_load_reject_a_foreign_seed() {
        let mut store = DistrictPolicyStore::new(2026);
        store
            .create(&district_policy_request("Riverside"))
            .expect("create");
        let base = "{\n  \"seed\": 2026,\n  \"placements\": []\n}\n";
        let attached =
            attach_optional_policies(base.to_string(), Some(&store), 2026).expect("attach");
        assert_eq!(
            policies_from_world_json(&attached, 2026),
            Ok(Some(store.clone()))
        );

        let (code, message) = attach_optional_policies(base.to_string(), Some(&store), 2027)
            .expect_err("foreign save");
        assert_eq!(code, "seed_mismatch");
        assert!(message.contains("2026"), "{message}");

        let (code, message) = policies_from_world_json(&attached, 2027).expect_err("foreign load");
        assert_eq!(code, "seed_mismatch");
        assert!(message.contains("2027"), "{message}");
    }

    #[test]
    fn district_policy_world_save_and_load_reject_malformed_json() {
        let mut store = DistrictPolicyStore::new(2026);
        store
            .create(&district_policy_request("Riverside"))
            .expect("create");

        let (code, _) = attach_optional_policies("{\"seed\":".to_string(), Some(&store), 2026)
            .expect_err("malformed save");
        assert_eq!(code, "save_failed");
        let (code, _) = attach_optional_policies("[]".to_string(), Some(&store), 2026)
            .expect_err("non-object save");
        assert_eq!(code, "invalid_json");

        let (code, _) = policies_from_world_json("{\"seed\":", 2026).expect_err("malformed load");
        assert_eq!(code, "invalid_json");
        let (code, _) = policies_from_world_json("[]", 2026).expect_err("non-object load");
        assert_eq!(code, "invalid_json");
        let (code, _) =
            policies_from_world_json("{\"policies\": 7}", 2026).expect_err("scalar policies");
        assert_eq!(code, "invalid_policies");
        let (code, _) =
            policies_from_world_json("{\"policies\": null}", 2026).expect_err("null policies");
        assert_eq!(code, "invalid_policies");
    }

    #[test]
    fn district_policy_world_save_and_load_reject_an_invalid_registry() {
        let mut store = DistrictPolicyStore::new(2026);
        store
            .create(&district_policy_request("Riverside"))
            .expect("create");
        store.format_version = 99;
        let (code, message) =
            attach_optional_policies("{\"seed\": 2026}".to_string(), Some(&store), 2026)
                .expect_err("invalid save");
        assert_eq!(code, "invalid_policies");
        assert!(message.contains("99"), "{message}");
        let registry = serde_json::to_value(&store).expect("invalid store JSON");
        let envelope = serde_json::json!({ "seed": 2026, "policies": registry }).to_string();
        let (code, message) = policies_from_world_json(&envelope, 2026).expect_err("invalid load");
        assert_eq!(code, "invalid_policies");
        assert!(message.contains("99"), "{message}");

        let mut bad_counter = DistrictPolicyStore::new(2026);
        bad_counter.next_id = 0;
        let registry = serde_json::to_value(&bad_counter).expect("bad counter JSON");
        let envelope = serde_json::json!({ "seed": 2026, "policies": registry }).to_string();
        let (code, message) =
            policies_from_world_json(&envelope, 2026).expect_err("bad counter load");
        assert_eq!(code, "invalid_policies");
        assert!(message.contains("next_id"), "{message}");
    }

    #[test]
    fn district_policy_summary_reports_a_stable_empty_checksum() {
        assert_eq!(policy_summary(None), (0, 0));
        let first = DistrictPolicyStore::new(2026);
        let second = DistrictPolicyStore::new(2026);
        assert!(first.is_empty());
        assert_eq!(first.checksum(), second.checksum());
        assert_eq!(policy_summary(Some(&first)), (0, first.checksum()));
        assert_eq!(first.checksum(), DistrictPolicyStore::new(2026).checksum());
    }

    #[test]
    fn district_policy_world_load_replaces_or_clears_the_live_registry() {
        let mut previous = DistrictPolicyStore::new(2026);
        previous
            .create(&district_policy_request("Riverside"))
            .expect("previous");
        let mut live = Some(previous);
        assert_eq!(live.as_ref().map(DistrictPolicyStore::count), Some(1));

        let mut saved = DistrictPolicyStore::new(2026);
        let replacement = saved
            .create(&district_policy_request("Old Town"))
            .expect("replacement");
        let json = attach_optional_policies("{\"seed\": 2026}".to_string(), Some(&saved), 2026)
            .expect("attach");
        live = policies_from_world_json(&json, 2026).expect("load");
        let installed = live.as_ref().expect("installed registry");
        assert_eq!(installed, &saved);
        assert_eq!(installed.count(), 1);
        assert_eq!(installed.get(replacement.id), Some(&replacement));

        live = policies_from_world_json("{\"seed\": 2026}", 2026).expect("old load");
        assert!(live.is_none(), "an old save did not clear the registry");
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

    fn bridge_settlement(
        owner: Option<u64>,
        members: &[(u64, core_city::settlement::SettlementRole)],
        revision: u32,
    ) -> Settlement {
        Settlement {
            id: 7,
            name: "Harbour".to_string(),
            owner: "mayor".to_string(),
            center: [10, 20],
            radius_m: 64,
            owner_id: owner.map(|id| core_city::player::PlayerId::new(id).expect("player id")),
            members: members
                .iter()
                .map(|(id, role)| core_city::settlement::SettlementMember {
                    player: core_city::player::PlayerId::new(*id).expect("player id"),
                    role: *role,
                })
                .collect(),
            membership_revision: revision,
            tax_rate: 0,
        }
    }

    #[test]
    fn legacy_settlement_payload_stays_unowned_and_empty() {
        let payload = serde_json::to_value(settlement_payload(&bridge_settlement(None, &[], 0)))
            .expect("json");
        assert_eq!(payload["id"], 7);
        assert_eq!(payload["owner"], "mayor");
        assert_eq!(payload["owned"], false);
        assert!(payload["owner_id"].is_null());
        assert_eq!(payload["members"].as_array().expect("members").len(), 0);
        assert_eq!(payload["membership_revision"], 0);
        assert_eq!(payload["tax_rate"], 0);
        let checksum = payload["membership_checksum"].as_str().expect("checksum");
        assert_eq!(checksum.len(), 16);
        assert!(checksum.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn owned_settlement_payload_lists_sorted_members_and_bits() {
        use core_city::settlement::SettlementRole;
        let settlement = bridge_settlement(
            Some(1),
            &[
                (3, SettlementRole::Builder),
                (1, SettlementRole::Owner),
                (2, SettlementRole::Manager),
            ],
            4,
        );
        let payload = serde_json::to_value(settlement_payload(&settlement)).expect("json");
        assert_eq!(payload["owned"], true);
        assert_eq!(payload["owner_id"], 1);
        assert_eq!(payload["membership_revision"], 4);
        let members = payload["members"].as_array().expect("members");
        assert_eq!(members.len(), 3);
        let ids: Vec<_> = members
            .iter()
            .map(|member| member["player"].clone())
            .collect();
        let roles: Vec<_> = members
            .iter()
            .map(|member| member["role"].as_str().expect("role"))
            .collect();
        let bits: Vec<_> = members
            .iter()
            .map(|member| member["permissions"].as_u64().expect("bits"))
            .collect();
        assert_eq!(ids, vec![1, 2, 3]);
        assert_eq!(roles, vec!["owner", "manager", "builder"]);
        assert_eq!(bits, vec![15, 15, 1]);
    }

    #[test]
    fn settlement_payload_includes_the_data_only_tax_rate() {
        let mut settlement = bridge_settlement(
            Some(1),
            &[(1, core_city::settlement::SettlementRole::Owner)],
            1,
        );
        settlement.tax_rate = 35;
        let payload = serde_json::to_value(settlement_payload(&settlement)).expect("json");
        assert_eq!(payload["tax_rate"], 35);
        assert_eq!(payload["membership_revision"], 1);
    }

    #[test]
    fn tax_projection_preview_matches_the_d039_helper() {
        for (rate, expected) in [(0, 1.0), (5, 1.05), (100, 2.0)] {
            let (found_rate, combined, capped) =
                tax_projection_value(rate).expect("valid projection");
            assert_eq!(found_rate, u16::try_from(rate).expect("rate"));
            assert!((combined - expected).abs() < 1e-12, "rate {rate}");
            assert!(!capped);
        }
    }

    #[test]
    fn tax_projection_rejects_out_of_range_rates() {
        for rate in [-1, 101, i64::MAX] {
            assert_eq!(
                tax_projection_value(rate).expect_err("out of range").0,
                "tax.projection.rate_out_of_range"
            );
        }
    }

    #[test]
    fn settlement_membership_checksum_tracks_members_and_revision() {
        use core_city::settlement::SettlementRole;
        let first = bridge_settlement(Some(1), &[(1, SettlementRole::Owner)], 1);
        let same = bridge_settlement(Some(1), &[(1, SettlementRole::Owner)], 1);
        let added = bridge_settlement(
            Some(1),
            &[(1, SettlementRole::Owner), (2, SettlementRole::Builder)],
            2,
        );
        assert_eq!(
            settlement_membership_checksum(&first),
            settlement_membership_checksum(&same)
        );
        assert_ne!(
            settlement_membership_checksum(&first),
            settlement_membership_checksum(&added)
        );
        assert_eq!(
            settlement_membership_checksum(&first).len(),
            16,
            "checksum is 16 lowercase hex digits"
        );
    }

    #[test]
    fn settlement_payload_keeps_the_existing_record_keys() {
        let payload = serde_json::to_value(settlement_payload(&bridge_settlement(None, &[], 0)))
            .expect("json");
        for key in ["id", "name", "owner", "center", "radius_m"] {
            assert!(payload.get(key).is_some(), "missing legacy key {key}");
        }
        assert_eq!(payload["center"], serde_json::json!([10, 20]));
    }

    fn bridge_settlement_request(name: &str) -> SettlementRequest {
        SettlementRequest {
            name: name.to_string(),
            owner: "player".to_string(),
            center: [10, 20],
            radius_m: 64,
        }
    }

    fn page_settlement(id: u64) -> Settlement {
        Settlement {
            id,
            name: format!("S{id}"),
            owner: String::new(),
            center: [0, 0],
            radius_m: 1,
            owner_id: None,
            members: Vec::new(),
            membership_revision: 0,
            tax_rate: 0,
        }
    }

    fn page_ids(page: &SettlementPage<'_>) -> Vec<u64> {
        page.settlements
            .iter()
            .map(|settlement| settlement.id)
            .collect()
    }

    #[test]
    fn settlement_page_args_rejects_negative_values() {
        for (after_id, limit) in [(-1, 1), (0, -1), (i64::MIN, i64::MIN)] {
            let (code, _) = settlement_page_args(after_id, limit).expect_err("invalid page");
            assert_eq!(code, "invalid_page", "after_id={after_id} limit={limit}");
        }
    }

    #[test]
    fn settlement_page_args_clamps_limit_and_keeps_zero_as_all() {
        assert_eq!(settlement_page_args(0, 0), Ok((0, None)));
        assert_eq!(settlement_page_args(5, 1), Ok((5, Some(1))));
        assert_eq!(settlement_page_args(5, 256), Ok((5, Some(256))));
        assert_eq!(settlement_page_args(5, 257), Ok((5, Some(256))));
        assert_eq!(settlement_page_args(5, i64::MAX), Ok((5, Some(256))));
    }

    #[test]
    fn settlement_page_on_empty_store_is_empty() {
        let settlements: Vec<Settlement> = Vec::new();
        for (after_id, limit) in [(0, None), (7, Some(2)), (7, Some(256))] {
            let page = settlement_page(&settlements, after_id, limit);
            assert!(page.settlements.is_empty());
            assert_eq!(page.matched, 0);
            assert_eq!(page.next_after, 0);
            assert!(!page.has_more);
        }
    }

    #[test]
    fn settlement_page_cursor_is_exclusive_and_past_end_is_empty() {
        let settlements: Vec<Settlement> = (1..=3).map(page_settlement).collect();
        assert_eq!(
            page_ids(&settlement_page(&settlements, 0, None)),
            vec![1, 2, 3]
        );
        assert_eq!(
            page_ids(&settlement_page(&settlements, 1, None)),
            vec![2, 3]
        );
        assert_eq!(page_ids(&settlement_page(&settlements, 2, None)), vec![3]);
        let past = settlement_page(&settlements, 3, Some(2));
        assert!(past.settlements.is_empty());
        assert_eq!(past.matched, 0);
        assert_eq!(past.next_after, 0);
        assert!(!past.has_more);
        assert!(settlement_page(&settlements, u64::MAX, None)
            .settlements
            .is_empty());
    }

    #[test]
    fn settlement_page_reports_matched_next_after_and_has_more() {
        let settlements: Vec<Settlement> = (1..=3).map(page_settlement).collect();
        let first = settlement_page(&settlements, 0, Some(2));
        assert_eq!(page_ids(&first), vec![1, 2]);
        assert_eq!(first.matched, 3);
        assert_eq!(first.next_after, 2);
        assert!(first.has_more);
        let second = settlement_page(&settlements, first.next_after, Some(2));
        assert_eq!(page_ids(&second), vec![3]);
        assert_eq!(second.matched, 1);
        assert_eq!(second.next_after, 3);
        assert!(!second.has_more);
    }

    #[test]
    fn settlement_page_sorts_records_by_id() {
        let settlements = vec![page_settlement(3), page_settlement(1), page_settlement(2)];
        assert_eq!(
            page_ids(&settlement_page(&settlements, 0, Some(2))),
            vec![1, 2]
        );
    }

    #[test]
    fn settlement_pages_cover_every_record_once_and_keep_the_checksum() {
        for count in 0..=3 {
            let mut store = SettlementStore::new(2026);
            for index in 0..count {
                store
                    .found(&bridge_settlement_request(&format!("S{index}")))
                    .expect("found");
            }
            let all: Vec<u64> = store
                .settlements()
                .iter()
                .map(|settlement| settlement.id)
                .collect();
            let checksum = store.checksum();
            for limit in [Some(1), Some(2), Some(256), None] {
                let mut after_id = 0;
                let mut seen = Vec::new();
                let mut pages = 0;
                loop {
                    let page = settlement_page(store.settlements(), after_id, limit);
                    assert_eq!(
                        page.matched,
                        all.iter().filter(|id| **id > after_id).count(),
                        "matched after {after_id}"
                    );
                    assert_eq!(page.has_more, page.settlements.len() < page.matched);
                    seen.extend(page_ids(&page));
                    pages += 1;
                    assert!(pages <= 10, "paging did not terminate");
                    if !page.has_more {
                        break;
                    }
                    let next = page.next_after;
                    assert!(next > after_id, "cursor must advance");
                    after_id = next;
                }
                assert_eq!(seen, all, "count={count} limit={limit:?}");
                assert_eq!(
                    seen.len(),
                    seen.iter().collect::<std::collections::HashSet<_>>().len()
                );
                assert_eq!(store.checksum(), checksum, "paging must not mutate");
            }
        }
    }

    #[test]
    fn settlement_pages_match_the_unpaged_order() {
        let mut store = SettlementStore::new(2026);
        for index in 0..3 {
            store
                .found(&bridge_settlement_request(&format!("S{index}")))
                .expect("found");
        }
        let unpaged: Vec<u64> = store
            .settlements()
            .iter()
            .map(|settlement| settlement.id)
            .collect();
        let mut paged = Vec::new();
        let mut after_id = 0;
        loop {
            let page = settlement_page(store.settlements(), after_id, Some(1));
            paged.extend(page_ids(&page));
            if !page.has_more {
                break;
            }
            after_id = page.next_after;
        }
        assert_eq!(paged, unpaged);
    }

    #[test]
    fn settlement_mutations_use_the_owner_actor_and_core_rules() {
        let mut store = SettlementStore::new(2026);
        let unowned = store
            .found(&bridge_settlement_request("Old Harbour"))
            .expect("found");
        let owner = PlayerId::new(9).expect("owner");
        let member = PlayerId::new(10).expect("member");

        let claimed = settlement_claim_in_store(&mut store, unowned.id, owner, 0).expect("claim");
        assert_eq!(claimed.owner_id, Some(owner));
        assert_eq!(claimed.membership_revision, 1);

        let invited = settlement_invite_in_store(&mut store, unowned.id, member, "builder", 1)
            .expect("invite");
        assert_eq!(invited.membership_revision, 2);
        let promoted = settlement_set_role_in_store(&mut store, unowned.id, member, "manager", 2)
            .expect("promote");
        assert_eq!(promoted.membership_revision, 3);
        assert_eq!(promoted.role_of(member), Some(SettlementRole::Manager));

        let transferred =
            settlement_transfer_in_store(&mut store, unowned.id, member, 3).expect("transfer");
        assert_eq!(transferred.owner_id, Some(member));
        assert_eq!(transferred.role_of(owner), Some(SettlementRole::Manager));

        let taxed = settlement_set_tax_in_store(&mut store, unowned.id, 25).expect("tax");
        assert_eq!(taxed.tax_rate, 25);

        let removed = settlement_remove_member_in_store(&mut store, unowned.id, owner, 4)
            .expect("remove former owner");
        assert_eq!(removed.members.len(), 1);

        let error = settlement_invite_in_store(&mut store, unowned.id, member, "owner", 5)
            .expect_err("owner role is not invitable");
        assert_eq!(error.0, "settlement.role_invalid");
        let error = settlement_invite_in_store(
            &mut store,
            unowned.id,
            PlayerId::new(11).expect("player"),
            "builder",
            4,
        )
        .expect_err("stale revision");
        assert_eq!(error.0, "settlement.revision_conflict");

        let other = store
            .found(&bridge_settlement_request("Unowned"))
            .expect("second");
        let error = settlement_invite_in_store(&mut store, other.id, member, "builder", 0)
            .expect_err("unowned cannot invite");
        assert_eq!(error.0, "settlement.unowned");
    }

    #[test]
    fn settlement_rename_in_store_uses_owner_and_bumps_revision() {
        let mut store = SettlementStore::new(2026);
        let owner = core_city::player::PlayerId::new(1).expect("player id");
        let found = store
            .found_for_player(&bridge_settlement_request("Old Harbour"), owner)
            .expect("found");
        let before = store.checksum();
        let renamed = settlement_rename_in_store(
            &mut store,
            found.id,
            "New Harbour",
            found.membership_revision,
        )
        .expect("rename");
        assert_eq!(renamed.name, "New Harbour");
        assert_eq!(renamed.membership_revision, found.membership_revision + 1);
        assert_eq!(store.get(found.id).expect("settlement").name, "New Harbour");
        assert_ne!(store.checksum(), before);
    }

    #[test]
    fn settlement_rename_in_store_rejects_stale_revision_without_mutation() {
        let mut store = SettlementStore::new(2026);
        let owner = core_city::player::PlayerId::new(1).expect("player id");
        let found = store
            .found_for_player(&bridge_settlement_request("Old Harbour"), owner)
            .expect("found");
        let before = store.checksum();
        let (code, _) = settlement_rename_in_store(
            &mut store,
            found.id,
            "New Harbour",
            found.membership_revision - 1,
        )
        .expect_err("stale revision");
        assert_eq!(code, "settlement.revision_conflict");
        assert_eq!(store.get(found.id).expect("settlement").name, "Old Harbour");
        assert_eq!(store.checksum(), before);
    }

    #[test]
    fn settlement_rename_in_store_rejects_unowned_without_mutation() {
        let mut store = SettlementStore::new(2026);
        let found = store
            .found(&bridge_settlement_request("Old Harbour"))
            .expect("found");
        let before = store.checksum();
        let (code, _) = settlement_rename_in_store(
            &mut store,
            found.id,
            "New Harbour",
            found.membership_revision,
        )
        .expect_err("unowned");
        assert_eq!(code, "settlement.unowned");
        assert_eq!(store.get(found.id).expect("settlement").name, "Old Harbour");
        assert_eq!(store.checksum(), before);
    }

    #[test]
    fn settlement_rename_in_store_rejects_unknown_id() {
        let mut store = SettlementStore::new(2026);
        let before = store.checksum();
        let (code, _) =
            settlement_rename_in_store(&mut store, 99, "New Harbour", 1).expect_err("unknown");
        assert_eq!(code, "settlement.unknown");
        assert_eq!(store.checksum(), before);
    }

    #[test]
    fn settlement_rename_args_rejects_negative_and_overflowing_values() {
        assert_eq!(
            settlement_rename_args(-1, 0).expect_err("negative id").0,
            "invalid_id"
        );
        assert_eq!(
            settlement_rename_args(1, -1)
                .expect_err("negative revision")
                .0,
            "invalid_revision"
        );
        assert_eq!(
            settlement_rename_args(1, i64::from(u32::MAX) + 1)
                .expect_err("overflowing revision")
                .0,
            "invalid_revision"
        );
        assert_eq!(
            settlement_rename_args(1, i64::from(u32::MAX)).expect("valid"),
            (1, u32::MAX)
        );
    }

    #[test]
    fn settlement_release_args_reject_negative_and_overflowing_values() {
        assert_eq!(
            settlement_mutation_args(-1, 0).expect_err("negative id").0,
            "invalid_id"
        );
        assert_eq!(
            settlement_mutation_args(1, -1)
                .expect_err("negative revision")
                .0,
            "invalid_revision"
        );
        assert_eq!(
            settlement_mutation_args(1, i64::from(u32::MAX) + 1)
                .expect_err("overflowing revision")
                .0,
            "invalid_revision"
        );
        assert_eq!(
            settlement_mutation_args(1, i64::from(u32::MAX)).expect("valid"),
            (1, u32::MAX)
        );
    }

    #[test]
    fn settlement_release_in_store_clears_owner_and_members() {
        use core_city::settlement::SettlementRole;
        let mut store = SettlementStore::new(2026);
        let owner = core_city::player::PlayerId::new(1).expect("player id");
        let member = core_city::player::PlayerId::new(2).expect("player id");
        let found = store
            .found_for_player(&bridge_settlement_request("Old Harbour"), owner)
            .expect("found");
        store
            .invite(
                owner,
                found.id,
                member,
                SettlementRole::Manager,
                found.membership_revision,
            )
            .expect("invite");
        let before = store.checksum();
        let revision = store.get(found.id).expect("settlement").membership_revision;
        let released =
            settlement_release_in_store(&mut store, found.id, revision).expect("release");
        assert!(!released.is_owned());
        assert_eq!(released.owner_id, None);
        assert!(released.members.is_empty());
        assert_eq!(released.membership_revision, revision + 1);
        assert_ne!(store.checksum(), before);
    }

    #[test]
    fn settlement_release_in_store_rejects_unknown_stale_and_unowned() {
        use core_city::settlement::SettlementRole;
        let mut store = SettlementStore::new(2026);
        let owner = core_city::player::PlayerId::new(1).expect("player id");
        let member = core_city::player::PlayerId::new(2).expect("player id");
        let found = store
            .found_for_player(&bridge_settlement_request("Old Harbour"), owner)
            .expect("found");
        store
            .invite(
                owner,
                found.id,
                member,
                SettlementRole::Manager,
                found.membership_revision,
            )
            .expect("invite");
        let before = store.checksum();
        let revision = store.get(found.id).expect("settlement").membership_revision;

        let (code, _) = settlement_release_in_store(&mut store, 99, revision).expect_err("unknown");
        assert_eq!(code, "settlement.unknown");
        let (code, _) =
            settlement_release_in_store(&mut store, found.id, revision - 1).expect_err("stale");
        assert_eq!(code, "settlement.revision_conflict");
        assert_eq!(store.checksum(), before);

        settlement_release_in_store(&mut store, found.id, revision).expect("release");
        let after_release = store.checksum();
        let (code, _) = settlement_release_in_store(&mut store, found.id, revision + 1)
            .expect_err("already released");
        assert_eq!(code, "settlement.unowned");
        assert_eq!(store.checksum(), after_release);
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

    fn test_policy(
        seed: u64,
        name: &str,
        center: [f32; 2],
        radius_m: f32,
        tax_multiplier: f32,
    ) -> DistrictPolicyStore {
        let mut store = DistrictPolicyStore::new(seed);
        let request = DistrictPolicyRequest {
            name: name.to_string(),
            center,
            radius_m,
            tax_multiplier,
            service_funding: 1.0,
            construction_allowed: true,
            blueprint_allowlist: BTreeSet::new(),
        };
        store.create(&request).expect("policy");
        store
    }

    fn test_household(residents: u32, workers: u32) -> Household {
        Household {
            capacity: residents.max(workers),
            residents,
            workers,
            food: 1.0,
            water: 1.0,
            rest: 1.0,
            commute_m: 0.0,
            commute_time_s: 0.0,
            income_per_day: 0.0,
            home: [0.0, 0.0],
            workplace_id: None,
        }
    }

    fn test_runtime(
        seed: u64,
        households: &[(u64, u32, u32)],
        staffing: &[(u64, u32)],
    ) -> CityRuntime {
        let mut runtime = CityRuntime::new(seed);
        for &(id, residents, workers) in households {
            runtime
                .households
                .insert(id, test_household(residents, workers));
        }
        for &(id, workers) in staffing {
            runtime.staffing.insert(id, workers);
        }
        runtime
    }

    fn home_centers(entries: &[(u64, [f32; 2])]) -> BTreeMap<u64, [f32; 2]> {
        entries.iter().copied().collect()
    }

    #[test]
    fn missing_or_empty_policy_store_keeps_taxes_neutral() {
        let runtime = test_runtime(7, &[(1, 4, 2)], &[(10, 2)]);
        let homes = home_centers(&[(1, [0.0, 0.0])]);
        let workplaces = [Workplace {
            id: 10,
            slots: 2,
            center: [5.0, 0.0],
        }];
        let neutral = policy_tax_weights(
            None,
            7,
            Some(7),
            Some(&runtime),
            &homes,
            &workplaces,
            &runtime.staffing,
        );
        assert_eq!(neutral, PolicyTaxWeights::ONE);
        assert!(neutral.is_neutral());

        let empty = DistrictPolicyStore::new(7);
        assert_eq!(
            policy_tax_weights(
                Some(&empty),
                7,
                Some(7),
                Some(&runtime),
                &homes,
                &workplaces,
                &runtime.staffing,
            ),
            PolicyTaxWeights::ONE
        );

        // A city with no residents or staff also has no denominators.
        let vacant = test_runtime(7, &[], &[]);
        let store = test_policy(7, "Tax", [0.0, 0.0], 100.0, 2.0);
        assert_eq!(
            policy_tax_weights(
                Some(&store),
                7,
                Some(7),
                Some(&vacant),
                &BTreeMap::new(),
                &[],
                &BTreeMap::new(),
            ),
            PolicyTaxWeights::ONE
        );
    }

    #[test]
    fn a_doubled_district_policy_doubles_both_tax_sides() {
        let store = test_policy(7, "Doubled", [0.0, 0.0], 100.0, 2.0);
        let runtime = test_runtime(7, &[(1, 4, 3)], &[(10, 3)]);
        let homes = home_centers(&[(1, [10.0, 0.0])]);
        let workplaces = [Workplace {
            id: 10,
            slots: 4,
            center: [-10.0, 0.0],
        }];
        let weights = policy_tax_weights(
            Some(&store),
            7,
            Some(7),
            Some(&runtime),
            &homes,
            &workplaces,
            &runtime.staffing,
        );
        assert_eq!(weights, PolicyTaxWeights::new(2.0, 2.0).expect("weights"));
        assert_eq!(weights.resident(), 2.0);
        assert_eq!(weights.business(), 2.0);

        let mut neutral = Economy::new(7);
        let base = neutral.tick(core_city::DAY_SECONDS, 4, 3, 1);
        let mut weighted = Economy::new(7);
        let ticked = weighted.tick_with_policy_taxes(
            core_city::DAY_SECONDS,
            4,
            3,
            1,
            InfrastructureCosts::ZERO,
            weights,
        );
        assert!((ticked.income - base.income * 2.0).abs() < 1e-9);
        assert!((ticked.expenses - base.expenses).abs() < 1e-9);
    }

    #[test]
    fn homes_and_workplaces_outside_every_policy_stay_neutral() {
        let store = test_policy(7, "Elsewhere", [1_000.0, 0.0], 16.0, 4.0);
        let runtime = test_runtime(7, &[(1, 4, 3)], &[(10, 3)]);
        let homes = home_centers(&[(1, [0.0, 0.0])]);
        let workplaces = [Workplace {
            id: 10,
            slots: 4,
            center: [50.0, 0.0],
        }];
        let weights = policy_tax_weights(
            Some(&store),
            7,
            Some(7),
            Some(&runtime),
            &homes,
            &workplaces,
            &runtime.staffing,
        );
        assert_eq!(weights, PolicyTaxWeights::ONE);
    }

    #[test]
    fn overlapping_districts_resolve_to_the_smallest_policy_id() {
        let mut store = test_policy(7, "Outer", [0.0, 0.0], 100.0, 1.0);
        let inner = DistrictPolicyRequest {
            name: "Inner".to_string(),
            center: [100.0, 0.0],
            radius_m: 50.0,
            tax_multiplier: 4.0,
            service_funding: 1.0,
            construction_allowed: true,
            blueprint_allowlist: BTreeSet::new(),
        };
        store.create(&inner).expect("inner");
        assert_eq!(store.count(), 2);
        let runtime = test_runtime(7, &[(1, 10, 2)], &[(10, 2)]);
        // The home sits inside both circles, so the older id-1 policy (1.0)
        // wins. The workplace is only inside the newer id-2 circle (4.0).
        let homes = home_centers(&[(1, [0.0, 0.0])]);
        let workplaces = [Workplace {
            id: 10,
            slots: 2,
            center: [140.0, 0.0],
        }];
        let weights = policy_tax_weights(
            Some(&store),
            7,
            Some(7),
            Some(&runtime),
            &homes,
            &workplaces,
            &runtime.staffing,
        );
        assert_eq!(weights.resident(), 1.0);
        assert_eq!(weights.business(), 4.0);
    }

    #[test]
    fn split_household_weights_each_workplace_staffing() {
        // The household remembers workplace 10 as its majority destination,
        // but its three workers are staffed at two sites. The business side
        // weights each workplace's staff at that workplace's own centre.
        let store = test_policy(7, "Doubled", [0.0, 0.0], 20.0, 2.0);
        let mut runtime = test_runtime(7, &[(1, 4, 3)], &[(10, 2), (11, 1)]);
        runtime
            .households
            .get_mut(&1)
            .expect("household")
            .workplace_id = Some(10);
        let homes = home_centers(&[(1, [0.0, 0.0])]);
        let workplaces = [
            Workplace {
                id: 10,
                slots: 2,
                center: [5.0, 0.0],
            },
            Workplace {
                id: 11,
                slots: 1,
                center: [200.0, 0.0],
            },
        ];
        let weights = policy_tax_weights(
            Some(&store),
            7,
            Some(7),
            Some(&runtime),
            &homes,
            &workplaces,
            &runtime.staffing,
        );
        assert_eq!(weights.resident(), 2.0);
        // (2 staff * 2.0 + 1 staff * 1.0) / 3 staff.
        assert!((weights.business() - 5.0 / 3.0).abs() < 1e-9);
        assert_ne!(weights.business(), 2.0);
    }

    #[test]
    fn foreign_registry_placement_or_population_seed_keeps_taxes_neutral() {
        let store = test_policy(8, "Foreign", [0.0, 0.0], 100.0, 2.0);
        let runtime = test_runtime(7, &[(1, 4, 3)], &[(10, 3)]);
        let homes = home_centers(&[(1, [0.0, 0.0])]);
        let workplaces = [Workplace {
            id: 10,
            slots: 3,
            center: [5.0, 0.0],
        }];
        assert_eq!(
            policy_tax_weights(
                Some(&store),
                7,
                Some(7),
                Some(&runtime),
                &homes,
                &workplaces,
                &runtime.staffing,
            ),
            PolicyTaxWeights::ONE
        );

        let store = test_policy(7, "Local", [0.0, 0.0], 100.0, 2.0);
        assert_eq!(
            policy_tax_weights(
                Some(&store),
                7,
                Some(8),
                Some(&runtime),
                &homes,
                &workplaces,
                &runtime.staffing,
            ),
            PolicyTaxWeights::ONE
        );

        let foreign_population = test_runtime(8, &[(1, 4, 3)], &[(10, 3)]);
        assert_eq!(
            policy_tax_weights(
                Some(&store),
                7,
                Some(7),
                Some(&foreign_population),
                &homes,
                &workplaces,
                &foreign_population.staffing,
            ),
            PolicyTaxWeights::ONE
        );
        assert_eq!(
            policy_tax_weights(
                Some(&store),
                7,
                Some(7),
                None,
                &homes,
                &workplaces,
                &BTreeMap::new(),
            ),
            PolicyTaxWeights::ONE
        );
    }

    fn service_policy(
        seed: u64,
        name: &str,
        center: [f32; 2],
        radius_m: f32,
        service_funding: f32,
    ) -> DistrictPolicyStore {
        let mut store = DistrictPolicyStore::new(seed);
        let request = DistrictPolicyRequest {
            name: name.to_string(),
            center,
            radius_m,
            tax_multiplier: 1.0,
            service_funding,
            construction_allowed: true,
            blueprint_allowlist: BTreeSet::new(),
        };
        store.create(&request).expect("policy");
        store
    }

    fn service_store(seed: u64, sites: &[(u64, [f32; 3], bool)]) -> ServiceStore {
        let mut store = ServiceStore::new(seed);
        for &(id, center, operational) in sites {
            let mut site =
                core_city::ServiceSite::default_for(ServiceKind::School, ServiceTier::Basic);
            site.center = center;
            site.build_days_remaining = if operational { 0.0 } else { 1.0 };
            store.sites.insert(id, site);
        }
        store
    }

    #[test]
    fn missing_empty_or_foreign_policy_store_keeps_service_funding_neutral() {
        let services = service_store(7, &[(1, [0.0, 0.0, 0.0], true)]);
        assert_eq!(
            policy_service_funding_weight(None, 7, Some(7), Some(7), Some(&services)),
            PolicyServiceWeight::ONE
        );

        let empty = DistrictPolicyStore::new(7);
        assert_eq!(
            policy_service_funding_weight(Some(&empty), 7, Some(7), Some(7), Some(&services)),
            PolicyServiceWeight::ONE
        );

        // A registry without a completed service site has no denominator.
        let policies = service_policy(7, "Funded", [0.0, 0.0], 100.0, 2.0);
        assert_eq!(
            policy_service_funding_weight(Some(&policies), 7, Some(7), Some(7), None),
            PolicyServiceWeight::ONE
        );
        assert_eq!(
            policy_service_funding_weight(
                Some(&policies),
                7,
                Some(7),
                Some(7),
                Some(&ServiceStore::new(7)),
            ),
            PolicyServiceWeight::ONE
        );
    }

    #[test]
    fn half_funding_policy_halves_the_charged_service_term() {
        let policies = service_policy(7, "Half", [0.0, 0.0], 100.0, 0.5);
        let services = service_store(
            7,
            &[(1, [10.0, 0.0, 0.0], true), (2, [-10.0, 0.0, 0.0], true)],
        );
        let weight =
            policy_service_funding_weight(Some(&policies), 7, Some(7), Some(7), Some(&services));
        assert_eq!(weight, PolicyServiceWeight::new(0.5).expect("weight"));

        let costs = InfrastructureCosts::new(100.0, 200.0).expect("costs");
        let mut neutral = Economy::new(7);
        let base = neutral.tick_with_infrastructure_costs(core_city::DAY_SECONDS, 0, 0, 0, costs);
        let mut weighted = Economy::new(7);
        let tick = weighted.tick_with_policy_weights(
            core_city::DAY_SECONDS,
            0,
            0,
            0,
            costs,
            PolicyTaxWeights::ONE,
            weight,
        );
        assert!((base.infrastructure - 300.0).abs() < 1e-9);
        assert!((tick.infrastructure - 200.0).abs() < 1e-9);
        assert!((tick.expenses - (base.expenses - 100.0)).abs() < 1e-9);
    }

    #[test]
    fn double_funding_policy_doubles_the_charged_service_term() {
        let policies = service_policy(7, "Doubled", [0.0, 0.0], 100.0, 2.0);
        let services = service_store(7, &[(1, [10.0, 0.0, 0.0], true)]);
        let weight =
            policy_service_funding_weight(Some(&policies), 7, Some(7), Some(7), Some(&services));
        assert_eq!(weight, PolicyServiceWeight::new(2.0).expect("weight"));

        let costs = InfrastructureCosts::new(100.0, 200.0).expect("costs");
        let mut economy = Economy::new(7);
        let tick = economy.tick_with_policy_weights(
            core_city::DAY_SECONDS,
            0,
            0,
            0,
            costs,
            PolicyTaxWeights::ONE,
            weight,
        );
        assert!((tick.infrastructure - 500.0).abs() < 1e-9);
    }

    #[test]
    fn policies_covering_only_homes_or_industry_do_not_change_service_funding() {
        // A home/industry policy at the origin must not scale a service site
        // that stands outside its circle.
        let policies = service_policy(7, "Old Town", [0.0, 0.0], 16.0, 0.5);
        let services = service_store(7, &[(1, [400.0, 0.0, 0.0], true)]);
        assert_eq!(
            policy_service_funding_weight(Some(&policies), 7, Some(7), Some(7), Some(&services)),
            PolicyServiceWeight::ONE
        );
    }

    #[test]
    fn only_completed_service_sites_contribute_to_the_funding_weight() {
        let policies = service_policy(7, "Doubled", [0.0, 0.0], 100.0, 2.0);
        let mixed = service_store(
            7,
            &[(1, [0.0, 0.0, 0.0], true), (2, [50.0, 0.0, 0.0], false)],
        );
        assert_eq!(
            policy_service_funding_weight(Some(&policies), 7, Some(7), Some(7), Some(&mixed)),
            PolicyServiceWeight::new(2.0).expect("weight")
        );

        let building = service_store(7, &[(1, [0.0, 0.0, 0.0], false)]);
        assert_eq!(
            policy_service_funding_weight(Some(&policies), 7, Some(7), Some(7), Some(&building)),
            PolicyServiceWeight::ONE
        );
    }

    #[test]
    fn service_funding_weight_is_the_mean_over_completed_sites_and_repeats() {
        let mut policies = service_policy(7, "Half", [0.0, 0.0], 100.0, 0.5);
        let doubled = DistrictPolicyRequest {
            name: "Doubled".to_string(),
            center: [1_000.0, 0.0],
            radius_m: 100.0,
            tax_multiplier: 1.0,
            service_funding: 2.0,
            construction_allowed: true,
            blueprint_allowlist: BTreeSet::new(),
        };
        policies.create(&doubled).expect("second policy");
        let services = service_store(
            7,
            &[
                (1, [10.0, 0.0, 0.0], true),
                (2, [1_000.0, 0.0, 0.0], true),
                (3, [2_000.0, 0.0, 0.0], true),
            ],
        );
        let first =
            policy_service_funding_weight(Some(&policies), 7, Some(7), Some(7), Some(&services));
        let second =
            policy_service_funding_weight(Some(&policies), 7, Some(7), Some(7), Some(&services));
        // (0.5 + 2.0 + neutral 1.0) / 3 completed sites.
        assert!((first.value() - 3.5 / 3.0).abs() < 1e-9);
        assert_eq!(first, second);

        // Overlapping circles use the smallest policy id, not the newest one.
        let mut overlapping = service_policy(7, "Outer", [0.0, 0.0], 100.0, 0.5);
        let inner = DistrictPolicyRequest {
            name: "Inner".to_string(),
            center: [0.0, 0.0],
            radius_m: 16.0,
            tax_multiplier: 1.0,
            service_funding: 2.0,
            construction_allowed: true,
            blueprint_allowlist: BTreeSet::new(),
        };
        overlapping.create(&inner).expect("inner");
        let site = service_store(7, &[(1, [0.0, 0.0, 0.0], true)]);
        assert_eq!(
            policy_service_funding_weight(Some(&overlapping), 7, Some(7), Some(7), Some(&site)),
            PolicyServiceWeight::new(0.5).expect("weight")
        );
    }

    #[test]
    fn foreign_seeds_and_outside_circles_keep_service_funding_neutral() {
        let services = service_store(7, &[(1, [0.0, 0.0, 0.0], true)]);

        let foreign_registry = service_policy(8, "Foreign", [0.0, 0.0], 100.0, 2.0);
        assert_eq!(
            policy_service_funding_weight(
                Some(&foreign_registry),
                7,
                Some(7),
                Some(7),
                Some(&services),
            ),
            PolicyServiceWeight::ONE
        );

        let local = service_policy(7, "Local", [0.0, 0.0], 100.0, 2.0);
        assert_eq!(
            policy_service_funding_weight(Some(&local), 7, Some(8), Some(7), Some(&services)),
            PolicyServiceWeight::ONE
        );
        assert_eq!(
            policy_service_funding_weight(Some(&local), 7, Some(7), Some(8), Some(&services)),
            PolicyServiceWeight::ONE
        );

        let foreign_services = service_store(8, &[(1, [0.0, 0.0, 0.0], true)]);
        assert_eq!(
            policy_service_funding_weight(
                Some(&local),
                7,
                Some(7),
                Some(7),
                Some(&foreign_services),
            ),
            PolicyServiceWeight::ONE
        );

        let outside = service_store(7, &[(1, [5_000.0, 0.0, 0.0], true)]);
        assert_eq!(
            policy_service_funding_weight(Some(&local), 7, Some(7), Some(7), Some(&outside)),
            PolicyServiceWeight::ONE
        );
    }

    #[test]
    fn non_finite_service_funding_falls_back_to_neutral() {
        let mut policies = service_policy(7, "Local", [0.0, 0.0], 100.0, 1.0);
        policies
            .policies
            .get_mut(&1)
            .expect("policy")
            .service_funding = f32::NAN;
        let services = service_store(7, &[(1, [0.0, 0.0, 0.0], true)]);
        assert_eq!(
            policy_service_funding_weight(Some(&policies), 7, Some(7), Some(7), Some(&services)),
            PolicyServiceWeight::ONE
        );
    }

    const BASE_MATERIALS: usize = 8;
    const BASE_ROOMS: usize = 7;
    const BASE_PROPS: usize = 8;
    const BASE_TOTAL: usize = 23;
    const BASE_CHECKSUM: u64 = 0xc2df_0f24_2f5e_78d2;

    const EXAMPLE_MANIFEST: &str = include_str!("../../../content/mods/example.mod.json");
    const EXAMPLE_MATERIALS: &str = include_str!("../../../content/mods/example/materials.json");
    const EXAMPLE_ROOMS: &str = include_str!("../../../content/mods/example/rooms.json");
    const EXAMPLE_PROPS: &str = include_str!("../../../content/mods/example/props.json");
    const EXAMPLE_PACK_CHECKSUM: u64 = 0xad4c_6458_69a4_11ed;
    const EXAMPLE_MERGED_MATERIALS: usize = 11;
    const EXAMPLE_MERGED_ROOMS: usize = 9;
    const EXAMPLE_MERGED_PROPS: usize = 11;
    const EXAMPLE_MERGED_TOTAL: usize = 31;
    const EXAMPLE_MERGED_CHECKSUM: u64 = 0x330a_538d_0780_5b50;

    /// Self-cleaning mod directory under the OS temp directory.
    struct TempContentDir {
        path: PathBuf,
    }

    impl TempContentDir {
        fn new(label: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let unique = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "townsmith-content-{}-{unique}-{label}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create temp content directory");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn as_str(&self) -> &str {
            self.path.to_str().expect("temp content path is UTF-8")
        }
    }

    impl Drop for TempContentDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn write_content_file(root: &Path, relative: &str, contents: &[u8]) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create content parent directory");
        }
        std::fs::write(path, contents).expect("write content file");
    }

    fn write_example_pack(root: &Path) {
        write_content_file(root, "mod.json", EXAMPLE_MANIFEST.as_bytes());
        for (relative, body) in [
            ("content/materials.json", EXAMPLE_MATERIALS),
            ("content/rooms.json", EXAMPLE_ROOMS),
            ("content/props.json", EXAMPLE_PROPS),
        ] {
            write_content_file(root, relative, body.as_bytes());
        }
    }

    fn manifest_document(id: &str, content: (&str, &str, &str)) -> String {
        serde_json::json!({
            "format_version": 1,
            "id": id,
            "name": "Runtime content test",
            "version": "1.0.0",
            "game_version": ">=0.1.0 <0.2.0",
            "content": {
                "materials": content.0,
                "rooms": content.1,
                "props": content.2
            }
        })
        .to_string()
    }

    fn material_value(id: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "name_key": "content.material.runtime_test",
            "category": "wood",
            "density_kg_m3": 700,
            "color": [0.5, 0.4, 0.2],
            "cost_per_litre": 0.1,
            "tags": ["test"]
        })
    }

    fn material_document(id: &str) -> String {
        serde_json::json!({
            "format_version": 1,
            "materials": [material_value(id)]
        })
        .to_string()
    }

    fn room_document(id: &str) -> String {
        serde_json::json!({
            "format_version": 1,
            "rooms": [{
                "id": id,
                "name_key": "content.room.runtime_test",
                "category": "living",
                "min_area_m2": 9.0,
                "required_tags": ["bed"]
            }]
        })
        .to_string()
    }

    fn prop_document(id: &str, room: &str) -> String {
        serde_json::json!({
            "format_version": 1,
            "props": [{
                "id": id,
                "name_key": "content.prop.runtime_test",
                "footprint_m": [1.6, 2.0],
                "height_m": 0.6,
                "cost": 10.0,
                "allowed_rooms": [room],
                "tags": ["sleep"]
            }]
        })
        .to_string()
    }

    fn write_single_pack(root: &Path, id: &str, material: &str, room: &str, prop: &str) {
        write_content_file(
            root,
            "mod.json",
            manifest_document(id, ("materials.json", "rooms.json", "props.json")).as_bytes(),
        );
        write_content_file(
            root,
            "materials.json",
            material_document(material).as_bytes(),
        );
        write_content_file(root, "rooms.json", room_document(room).as_bytes());
        write_content_file(root, "props.json", prop_document(prop, room).as_bytes());
    }

    fn assert_content_load_fails(content: &mut RuntimeContent, raw: &str, code: &str) {
        let before = content.summary().expect("summary before failed load");
        let first = content.load_dir(raw).expect_err("load must fail");
        let second = content.load_dir(raw).expect_err("load must fail again");
        assert_eq!(first, second, "error must be stable");
        assert_eq!(first.code, code, "{}", first.message);
        assert_eq!(
            content.summary().expect("summary after failed load"),
            before
        );
    }

    #[test]
    fn content_registry_starts_with_the_committed_base_content() {
        let first = RuntimeContent::base();
        let summary = first.summary().expect("base summary");
        assert_eq!(
            summary,
            RuntimeContent::base().summary().expect("base summary")
        );
        assert_eq!(summary.format_version, ContentRegistry::FORMAT_VERSION);
        assert_eq!(summary.materials, BASE_MATERIALS);
        assert_eq!(summary.rooms, BASE_ROOMS);
        assert_eq!(summary.props, BASE_PROPS);
        assert_eq!(summary.total, BASE_TOTAL);
        assert_eq!(summary.packs, 0);
        assert_eq!(summary.checksum, BASE_CHECKSUM);
        assert!(summary.pack_id.is_empty());
        assert!(summary.namespace.is_empty());
        assert_eq!(summary.pack_checksum, 0);
        assert_eq!(summary.base_checksum, BASE_CHECKSUM);
        assert_eq!(
            summary.checksum,
            ContentRegistry::base().expect("base registry").checksum()
        );
        let registry = first.registry.as_ref().expect("base registry");
        assert!(registry.material("base:oak_plank").is_some());
        assert!(registry.room("base:bedroom").is_some());
        assert!(registry.prop("base:bed").is_some());
    }

    #[test]
    fn content_load_dir_merges_the_pack_after_base() {
        let dir = TempContentDir::new("valid");
        write_example_pack(dir.path());
        let mut content = RuntimeContent::base();
        let pack = content.load_dir(dir.as_str()).expect("pack loads");
        assert_eq!(pack.id, "example:copper_age");
        assert_eq!(pack.namespace, "example");
        assert_eq!(pack.checksum, EXAMPLE_PACK_CHECKSUM);
        let summary = content.summary().expect("summary");
        assert_eq!(summary.format_version, ContentRegistry::FORMAT_VERSION);
        assert_eq!(summary.materials, EXAMPLE_MERGED_MATERIALS);
        assert_eq!(summary.rooms, EXAMPLE_MERGED_ROOMS);
        assert_eq!(summary.props, EXAMPLE_MERGED_PROPS);
        assert_eq!(summary.total, EXAMPLE_MERGED_TOTAL);
        assert_eq!(summary.packs, 1);
        assert_eq!(summary.checksum, EXAMPLE_MERGED_CHECKSUM);
        assert_eq!(summary.pack_id, "example:copper_age");
        assert_eq!(summary.namespace, "example");
        assert_eq!(summary.pack_checksum, EXAMPLE_PACK_CHECKSUM);
        assert_eq!(summary.base_checksum, BASE_CHECKSUM);
        let registry = content.registry.as_ref().expect("merged registry");
        assert!(registry.material("base:oak_plank").is_some());
        assert!(registry.material("example:copper_sheet").is_some());
        assert!(registry.room("example:smelter_hall").is_some());
        assert!(registry.prop("example:copper_smelter").is_some());
    }

    #[test]
    fn content_active_identity_reports_base_and_one_pack() {
        let base = RuntimeContent::base();
        let identity = base.active_identity().expect("base identity");
        assert_eq!(identity.packs, 0);
        assert!(identity.pack_id.is_empty());
        assert_eq!(identity.pack_checksum, 0);
        assert_eq!(identity.registry_checksum, BASE_CHECKSUM);
        assert_eq!(identity.base_checksum, BASE_CHECKSUM);

        let dir = TempContentDir::new("identity-active");
        write_example_pack(dir.path());
        let mut content = RuntimeContent::base();
        content.load_dir(dir.as_str()).expect("pack loads");
        let identity = content.active_identity().expect("pack identity");
        assert_eq!(identity.packs, 1);
        assert_eq!(identity.pack_id, "example:copper_age");
        assert_eq!(identity.pack_checksum, EXAMPLE_PACK_CHECKSUM);
        assert_eq!(identity.registry_checksum, EXAMPLE_MERGED_CHECKSUM);
        assert_eq!(identity.base_checksum, BASE_CHECKSUM);

        content.clear().expect("clear");
        let identity = content.active_identity().expect("cleared identity");
        assert_eq!(identity.packs, 0);
        assert_eq!(identity.registry_checksum, BASE_CHECKSUM);
    }

    #[test]
    fn content_identity_save_load_cycle_retains_the_pack_binding() {
        let dir = TempContentDir::new("identity-cycle");
        write_example_pack(dir.path());
        let mut content = RuntimeContent::base();
        content.load_dir(dir.as_str()).expect("pack loads");
        let active = content.active_identity().expect("active identity");
        let store = PlacementStore::new(2026);
        let json = store.to_json_pretty().expect("world json");
        let saved = stamp_world_content(&json, &active, None).expect("content save");
        assert_eq!(saved.status_str(), "new");
        let value: serde_json::Value = serde_json::from_str(&saved.json).expect("v2 json");
        assert_eq!(value["format_version"], 2);
        assert_eq!(value["content"]["pack_id"], "example:copper_age");
        assert_eq!(value["content"]["pack_checksum"], "ad4c645869a411ed");
        assert_eq!(value["content"]["registry_checksum"], "330a538d07805b50");

        let verified = verify_world_content(&saved.json, &active).expect("matching load");
        assert_eq!(verified.status_str(), "match");
        assert_eq!(verified.identity.as_ref(), saved.identity.as_ref());
        let normalized = verified.normalized_json.as_deref().expect("v1 json");
        let reloaded = PlacementStore::from_json(normalized).expect("v1 records");
        assert_eq!(reloaded, store);
        assert_eq!(reloaded.checksum(), store.checksum());

        let base = RuntimeContent::base().active_identity().expect("base");
        let error = verify_world_content(&saved.json, &base).expect_err("base refuses the pack");
        assert_eq!(error.code, "content_pack_missing");

        let resaved =
            stamp_world_content(normalized, &active, verified.identity.as_ref()).expect("re-save");
        assert_eq!(resaved.status_str(), "match");
        assert_eq!(resaved.json, saved.json);
    }

    #[test]
    fn content_identity_save_requires_the_recorded_pack_after_clear() {
        let dir = TempContentDir::new("identity-clear");
        write_example_pack(dir.path());
        let mut content = RuntimeContent::base();
        content.load_dir(dir.as_str()).expect("pack loads");
        let active = content.active_identity().expect("active identity");
        let saved = stamp_world_content(
            &PlacementStore::new(2026).to_json_pretty().expect("json"),
            &active,
            None,
        )
        .expect("content save");

        content.clear().expect("clear the pack");
        let base = content.active_identity().expect("base identity");
        let error = stamp_world_content(&saved.json, &base, saved.identity.as_ref())
            .expect_err("base save must be refused");
        assert_eq!(error.code, "content_save_requires_pack");
    }

    #[test]
    fn content_read_only_error_has_a_stable_code() {
        let error = content_read_only_error();
        assert_eq!(error.code, "content_read_only");
        assert!(error.message.contains("restore the pack"));
    }

    #[test]
    fn content_load_dir_missing_directory_keeps_the_previous_registry() {
        let valid = TempContentDir::new("missing-valid");
        write_example_pack(valid.path());
        let mut content = RuntimeContent::base();
        content.load_dir(valid.as_str()).expect("valid pack loads");

        let empty = TempContentDir::new("missing-target");
        let missing = empty.path().join("absent");
        assert_content_load_fails(
            &mut content,
            missing.to_str().expect("missing path is UTF-8"),
            "content_directory_missing",
        );
        assert_eq!(
            content.summary().expect("summary").checksum,
            EXAMPLE_MERGED_CHECKSUM
        );
    }

    #[test]
    fn content_load_dir_non_directory_keeps_the_previous_registry() {
        let valid = TempContentDir::new("file-valid");
        write_example_pack(valid.path());
        let mut content = RuntimeContent::base();
        content.load_dir(valid.as_str()).expect("valid pack loads");

        let dir = TempContentDir::new("file-target");
        write_content_file(dir.path(), "mod.json", b"{}");
        let file = dir.path().join("mod.json");
        assert_content_load_fails(
            &mut content,
            file.to_str().expect("file path is UTF-8"),
            "content_not_a_directory",
        );
        assert_eq!(content.summary().expect("summary").packs, 1);
    }

    #[test]
    fn content_load_dir_rejects_relative_and_traversal_paths() {
        let mut content = RuntimeContent::base();
        assert_content_load_fails(&mut content, "mods/example", "invalid_content_path");
        let traversal = std::env::temp_dir().join("..").join("townsmith-content");
        assert_content_load_fails(
            &mut content,
            traversal.to_str().expect("traversal path is UTF-8"),
            "invalid_content_path",
        );
        assert_content_load_fails(&mut content, "   ", "invalid_content_path");
        let summary = content.summary().expect("summary");
        assert_eq!(summary.checksum, BASE_CHECKSUM);
        assert_eq!(summary.packs, 0);
    }

    #[test]
    fn content_load_dir_rejects_absolute_and_traversal_manifest_paths() {
        for (label, bad_path) in [
            ("manifest-parent", "../materials.json"),
            ("manifest-absolute", "/etc/materials.json"),
            ("manifest-nested-parent", "content/../../materials.json"),
        ] {
            let dir = TempContentDir::new(label);
            write_content_file(
                dir.path(),
                "mod.json",
                manifest_document("runtime:unsafe", (bad_path, "rooms.json", "props.json"))
                    .as_bytes(),
            );
            write_content_file(
                dir.path(),
                "materials.json",
                material_document("runtime:material").as_bytes(),
            );
            write_content_file(
                dir.path(),
                "rooms.json",
                room_document("runtime:room").as_bytes(),
            );
            write_content_file(
                dir.path(),
                "props.json",
                prop_document("runtime:prop", "runtime:room").as_bytes(),
            );

            let mut content = RuntimeContent::base();
            assert_content_load_fails(&mut content, dir.as_str(), "content_invalid_manifest");
        }
    }

    #[cfg(unix)]
    #[test]
    fn content_load_dir_rejects_a_symlink_escape() {
        use std::os::unix::fs::symlink;

        let outside = TempContentDir::new("symlink-outside");
        write_content_file(
            outside.path(),
            "materials.json",
            EXAMPLE_MATERIALS.as_bytes(),
        );
        let dir = TempContentDir::new("symlink-pack");
        write_content_file(dir.path(), "mod.json", EXAMPLE_MANIFEST.as_bytes());
        write_content_file(dir.path(), "content/rooms.json", EXAMPLE_ROOMS.as_bytes());
        write_content_file(dir.path(), "content/props.json", EXAMPLE_PROPS.as_bytes());
        symlink(
            outside.path().join("materials.json"),
            dir.path().join("content/materials.json"),
        )
        .expect("create symlink");

        let mut content = RuntimeContent::base();
        assert_content_load_fails(&mut content, dir.as_str(), "content_path_escape");
    }

    #[test]
    fn content_load_dir_rejects_an_oversized_file() {
        let dir = TempContentDir::new("oversized");
        write_content_file(
            dir.path(),
            "mod.json",
            manifest_document(
                "runtime:pack",
                ("materials.json", "rooms.json", "props.json"),
            )
            .as_bytes(),
        );
        write_content_file(
            dir.path(),
            "rooms.json",
            room_document("runtime:room").as_bytes(),
        );
        write_content_file(
            dir.path(),
            "props.json",
            prop_document("runtime:prop", "runtime:room").as_bytes(),
        );
        let mut oversized = material_document("runtime:material");
        let limit = usize::try_from(core_city::pack::MAX_PACK_FILE_BYTES).expect("limit fits");
        oversized.push_str(&" ".repeat(limit));
        write_content_file(dir.path(), "materials.json", oversized.as_bytes());

        let mut content = RuntimeContent::base();
        assert_content_load_fails(&mut content, dir.as_str(), "content_file_too_large");
    }

    #[test]
    fn content_load_dir_rejects_invalid_utf8() {
        let dir = TempContentDir::new("utf8");
        write_content_file(
            dir.path(),
            "mod.json",
            manifest_document(
                "runtime:pack",
                ("materials.json", "rooms.json", "props.json"),
            )
            .as_bytes(),
        );
        write_content_file(dir.path(), "materials.json", &[b'{', 0xff, b'}']);
        write_content_file(
            dir.path(),
            "rooms.json",
            room_document("runtime:room").as_bytes(),
        );
        write_content_file(
            dir.path(),
            "props.json",
            prop_document("runtime:prop", "runtime:room").as_bytes(),
        );

        let mut content = RuntimeContent::base();
        assert_content_load_fails(&mut content, dir.as_str(), "content_not_utf8");
    }

    #[test]
    fn content_load_dir_rejects_an_invalid_manifest() {
        let malformed = TempContentDir::new("manifest-malformed");
        write_content_file(malformed.path(), "mod.json", b"{");

        let mut content = RuntimeContent::base();
        assert_content_load_fails(&mut content, malformed.as_str(), "content_invalid_manifest");

        let unsupported = TempContentDir::new("manifest-version");
        let mut manifest: serde_json::Value = serde_json::from_str(&manifest_document(
            "runtime:pack",
            ("materials.json", "rooms.json", "props.json"),
        ))
        .expect("manifest parses");
        manifest["format_version"] = serde_json::json!(2);
        write_content_file(
            unsupported.path(),
            "mod.json",
            manifest.to_string().as_bytes(),
        );
        assert_content_load_fails(
            &mut content,
            unsupported.as_str(),
            "content_invalid_manifest",
        );
    }

    #[test]
    fn content_load_dir_rejects_duplicate_ids() {
        let collision = TempContentDir::new("duplicate-base");
        write_single_pack(
            collision.path(),
            "base",
            "base:oak_plank",
            "base:runtime_room",
            "base:runtime_prop",
        );
        let mut content = RuntimeContent::base();
        assert_content_load_fails(&mut content, collision.as_str(), "content_duplicate_id");

        let internal = TempContentDir::new("duplicate-pack");
        write_single_pack(
            internal.path(),
            "runtime:pack",
            "runtime:material",
            "runtime:room",
            "runtime:prop",
        );
        let duplicated = serde_json::json!({
            "format_version": 1,
            "materials": [
                material_value("runtime:material"),
                material_value("runtime:material")
            ]
        })
        .to_string();
        write_content_file(internal.path(), "materials.json", duplicated.as_bytes());
        assert_content_load_fails(&mut content, internal.as_str(), "content_invalid_content");
    }

    #[test]
    fn content_registry_clear_restores_the_base() {
        let dir = TempContentDir::new("clear");
        write_example_pack(dir.path());
        let mut content = RuntimeContent::base();
        content.load_dir(dir.as_str()).expect("pack loads");
        assert_eq!(content.summary().expect("summary").packs, 1);

        content.clear().expect("clear restores base");
        let summary = content.summary().expect("summary");
        assert_eq!(summary.materials, BASE_MATERIALS);
        assert_eq!(summary.rooms, BASE_ROOMS);
        assert_eq!(summary.props, BASE_PROPS);
        assert_eq!(summary.total, BASE_TOTAL);
        assert_eq!(summary.packs, 0);
        assert_eq!(summary.checksum, BASE_CHECKSUM);
    }

    #[test]
    fn content_load_dir_is_deterministic_across_repeated_loads() {
        let dir = TempContentDir::new("repeat");
        write_example_pack(dir.path());
        let mut content = RuntimeContent::base();
        let first_pack = content.load_dir(dir.as_str()).expect("first load");
        let first = content.summary().expect("first summary");
        let second_pack = content.load_dir(dir.as_str()).expect("second load");
        let second = content.summary().expect("second summary");

        assert_eq!(first_pack, second_pack);
        assert_eq!(first, second);
        assert_eq!(second.packs, 1);
        assert_eq!(second.checksum, EXAMPLE_MERGED_CHECKSUM);
    }

    #[test]
    fn failed_first_load_leaves_the_base_registry() {
        let dir = TempContentDir::new("partial");
        write_content_file(
            dir.path(),
            "mod.json",
            manifest_document(
                "runtime:pack",
                ("materials.json", "rooms.json", "props.json"),
            )
            .as_bytes(),
        );
        // The materials document is valid and would add one definition; the
        // invalid rooms document must stop the whole load before any merge.
        write_content_file(
            dir.path(),
            "materials.json",
            material_document("runtime:partial").as_bytes(),
        );
        write_content_file(dir.path(), "rooms.json", b"{");
        write_content_file(
            dir.path(),
            "props.json",
            prop_document("runtime:prop", "runtime:room").as_bytes(),
        );

        let mut content = RuntimeContent::base();
        assert_content_load_fails(&mut content, dir.as_str(), "content_invalid_content");
        let summary = content.summary().expect("summary");
        assert_eq!(summary.materials, BASE_MATERIALS);
        assert_eq!(summary.rooms, BASE_ROOMS);
        assert_eq!(summary.props, BASE_PROPS);
        assert_eq!(summary.total, BASE_TOTAL);
        assert_eq!(summary.packs, 0);
        assert_eq!(summary.checksum, BASE_CHECKSUM);
    }

    /// Self-cleaning root and package directory under the OS temp directory.
    struct TempBlueprintLibrary {
        path: PathBuf,
    }

    impl TempBlueprintLibrary {
        fn new(label: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let unique = NEXT.fetch_add(1, Ordering::Relaxed);
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos());
            let path = std::env::temp_dir().join(format!(
                "townsmith-blueprint-library-{}-{stamp}-{unique}-{label}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create temp blueprint directory");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn root(&self) -> PathBuf {
            self.path.join("library")
        }

        fn root_str(&self) -> String {
            self.root().to_string_lossy().into_owned()
        }

        fn create_root(&self) -> PathBuf {
            let root = self.root();
            std::fs::create_dir_all(&root).expect("create library root");
            root
        }

        fn package(&self, name: &str) -> PathBuf {
            self.path.join(name)
        }
    }

    impl Drop for TempBlueprintLibrary {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn write_bridge_package(path: &Path, id: &str) -> String {
        write_bridge_package_with_dependencies(path, id, &[], &[])
    }

    fn write_bridge_package_with_dependencies(
        path: &Path,
        id: &str,
        mods: &[&str],
        resource_packs: &[&str],
    ) -> String {
        let mut volume =
            VoxelVolume::new(VolumeConfig::new(1, 1, 1).expect("config")).expect("volume");
        volume
            .fill_box(
                VolumePos::new(0, 0, 0),
                VolumePos::new(4, 1, 4),
                EDITOR_FLOOR,
            )
            .expect("fill floor");
        let mut metadata = BlueprintMetadata::new(id, "Bridge Library House");
        metadata.dependencies.mods = mods.iter().map(|id| (*id).to_string()).collect();
        metadata.dependencies.resource_packs =
            resource_packs.iter().map(|id| (*id).to_string()).collect();
        let blueprint = Blueprint::new(metadata, volume);
        save_tsbp_with_hash(path, &blueprint).expect("save package")
    }

    #[test]
    fn blueprint_library_root_requires_an_absolute_path() {
        let abs = std::env::temp_dir().join("townsmith-library-root");
        assert!(blueprint_library_root_from_arg(&abs.to_string_lossy()).is_ok());
        assert_eq!(
            blueprint_library_root_from_arg("").expect_err("empty"),
            "blueprint library root is empty"
        );
        assert_eq!(
            blueprint_library_root_from_arg("   ").expect_err("blank"),
            "blueprint library root is empty"
        );
        assert_eq!(
            blueprint_library_root_from_arg("relative/library").expect_err("relative"),
            "blueprint library root must be absolute"
        );
        let traversal = std::env::temp_dir().join("one").join("..").join("two");
        assert_eq!(
            blueprint_library_root_from_arg(&traversal.to_string_lossy()).expect_err("traversal"),
            "blueprint library root must not contain '.' or '..'"
        );
    }

    #[test]
    fn blueprint_library_error_codes_are_stable() {
        let cases = [
            (
                BlueprintLibraryError::InvalidRoot {
                    path: PathBuf::from("x"),
                    reason: "test",
                },
                "invalid_blueprint_library_root",
            ),
            (
                BlueprintLibraryError::RootMissing {
                    path: PathBuf::from("x"),
                },
                "blueprint_library_root_missing",
            ),
            (
                BlueprintLibraryError::RootNotDirectory {
                    path: PathBuf::from("x"),
                },
                "blueprint_library_not_a_directory",
            ),
            (
                BlueprintLibraryError::InvalidHash {
                    hash: "x".to_string(),
                    reason: "test",
                },
                "invalid_blueprint_library_hash",
            ),
            (
                BlueprintLibraryError::InvalidPath {
                    path: PathBuf::from("x"),
                    reason: "test",
                },
                "invalid_blueprint_path",
            ),
            (
                BlueprintLibraryError::MissingFile {
                    path: PathBuf::from("x"),
                },
                "blueprint_library_missing_file",
            ),
            (
                BlueprintLibraryError::NotAFile {
                    path: PathBuf::from("x"),
                },
                "blueprint_library_not_a_file",
            ),
            (
                BlueprintLibraryError::InvalidPackage {
                    path: PathBuf::from("x"),
                    source: BlueprintError::Format("test".to_string()),
                },
                "blueprint_library_invalid_package",
            ),
            (
                BlueprintLibraryError::TooLarge {
                    path: PathBuf::from("x"),
                    bytes: 1,
                    limit: 0,
                },
                "blueprint_library_too_large",
            ),
            (
                BlueprintLibraryError::PathEscape {
                    path: PathBuf::from("x"),
                },
                "blueprint_library_path_escape",
            ),
            (
                BlueprintLibraryError::Collision {
                    hash: "blake3:test".to_string(),
                    path: PathBuf::from("x"),
                },
                "blueprint_library_collision",
            ),
            (
                BlueprintLibraryError::Io {
                    path: PathBuf::from("x"),
                    source: std::io::Error::new(std::io::ErrorKind::NotFound, "missing"),
                },
                "blueprint_library_missing_file",
            ),
            (
                BlueprintLibraryError::Io {
                    path: PathBuf::from("x"),
                    source: std::io::Error::other("io"),
                },
                "blueprint_library_io",
            ),
        ];
        for (error, code) in cases {
            assert_eq!(blueprint_library_error_code(&error), code, "{error}");
        }
    }

    #[test]
    fn blueprint_library_insert_then_resolve_and_summary() {
        let dir = TempBlueprintLibrary::new("insert");
        let root = dir.create_root();
        let package = dir.package("house.tsbp");
        let hash = write_bridge_package(&package, "local:bridge_house");

        let (insert, summary) =
            blueprint_library_insert_from_args(&dir.root_str(), &package.to_string_lossy())
                .expect("insert");
        assert!(insert.inserted);
        assert_eq!(insert.hash, hash);
        assert_eq!(
            insert.path,
            std::fs::canonicalize(root.join(&hash[7..9]).join(format!("{}.tsbp", &hash[7..])))
                .expect("canonical stored entry")
        );
        assert!(insert.path.is_file());
        assert_eq!(summary.count, 1);
        assert_eq!(
            summary.total_bytes,
            std::fs::metadata(&insert.path).unwrap().len()
        );
        assert_eq!(format!("{:016x}", summary.checksum).len(), 16);

        let resolved = blueprint_library_resolve_from_args(&dir.root_str(), &hash)
            .expect("resolve")
            .expect("stored");
        assert_eq!(resolved, insert.path);

        let read_back = blueprint_library_summary_from_arg(&dir.root_str()).expect("summary");
        assert_eq!(read_back, summary);
    }

    #[test]
    fn blueprint_library_duplicate_insert_is_a_noop() {
        let dir = TempBlueprintLibrary::new("duplicate");
        dir.create_root();
        let package = dir.package("house.tsbp");
        write_bridge_package(&package, "local:bridge_house");

        let (first, _) =
            blueprint_library_insert_from_args(&dir.root_str(), &package.to_string_lossy())
                .expect("first");
        let (second, summary) =
            blueprint_library_insert_from_args(&dir.root_str(), &package.to_string_lossy())
                .expect("second");
        assert!(first.inserted);
        assert!(!second.inserted);
        assert_eq!(first.hash, second.hash);
        assert_eq!(first.path, second.path);
        assert_eq!(summary.count, 1);
    }

    #[test]
    fn blueprint_library_missing_root_reports_a_stable_code() {
        let dir = TempBlueprintLibrary::new("missing-root");
        let root = dir.root();
        assert!(!root.exists());
        let (code, _) = blueprint_library_summary_from_arg(&dir.root_str()).expect_err("missing");
        assert_eq!(code, "blueprint_library_root_missing");
        let (code, _) = blueprint_library_insert_from_args(
            &dir.root_str(),
            &dir.package("house.tsbp").to_string_lossy(),
        )
        .expect_err("missing");
        assert_eq!(code, "blueprint_library_root_missing");
    }

    #[test]
    fn blueprint_library_non_directory_root_reports_a_stable_code() {
        let dir = TempBlueprintLibrary::new("file-root");
        let file = dir.path().join("not-a-directory");
        std::fs::write(&file, b"file").expect("file root");
        let (code, _) =
            blueprint_library_summary_from_arg(&file.to_string_lossy()).expect_err("non-directory");
        assert_eq!(code, "blueprint_library_not_a_directory");
    }

    #[test]
    fn blueprint_library_insert_rejects_missing_and_non_tsbp_packages() {
        let dir = TempBlueprintLibrary::new("bad-package-path");
        dir.create_root();
        let (code, _) = blueprint_library_insert_from_args(
            &dir.root_str(),
            &dir.package("missing.tsbp").to_string_lossy(),
        )
        .expect_err("missing package");
        assert_eq!(code, "blueprint_library_missing_file");

        let package = dir.package("house.tsbp");
        write_bridge_package(&package, "local:bridge_house");
        let renamed = dir.package("house.zip");
        std::fs::rename(&package, &renamed).expect("rename");
        let (code, _) =
            blueprint_library_insert_from_args(&dir.root_str(), &renamed.to_string_lossy())
                .expect_err("extension");
        assert_eq!(code, "invalid_blueprint_path");
    }

    #[test]
    fn blueprint_library_insert_rejects_an_invalid_package() {
        let dir = TempBlueprintLibrary::new("invalid-package");
        let root = dir.create_root();
        let package = dir.package("invalid.tsbp");
        std::fs::write(&package, b"not a zip").expect("invalid package");
        let (code, _) =
            blueprint_library_insert_from_args(&dir.root_str(), &package.to_string_lossy())
                .expect_err("invalid package");
        assert_eq!(code, "blueprint_library_invalid_package");
        let summary = blueprint_library_summary_from_arg(&dir.root_str()).expect("summary");
        assert_eq!(summary.count, 0);
        assert!(std::fs::read_dir(&root).expect("root").next().is_none());
    }

    #[test]
    fn blueprint_library_insert_rejects_an_oversized_package() {
        let dir = TempBlueprintLibrary::new("oversized");
        dir.create_root();
        let package = dir.package("oversized.tsbp");
        let file = std::fs::File::create(&package).expect("oversized package");
        file.set_len(core_blueprint::MAX_PACKAGE_BYTES + 1)
            .expect("set length");
        drop(file);
        let (code, _) =
            blueprint_library_insert_from_args(&dir.root_str(), &package.to_string_lossy())
                .expect_err("too large");
        assert_eq!(code, "blueprint_library_too_large");
        let summary = blueprint_library_summary_from_arg(&dir.root_str()).expect("summary");
        assert_eq!(summary.count, 0);
    }

    #[test]
    fn blueprint_library_resolve_rejects_invalid_hashes() {
        let dir = TempBlueprintLibrary::new("bad-hash");
        dir.create_root();
        let package = dir.package("house.tsbp");
        let hash = write_bridge_package(&package, "local:bridge_house");
        let hex = &hash[7..];
        let uppercase = format!("blake3:{}", hex.to_uppercase());
        let outside = package.to_string_lossy().into_owned();
        for bad in [
            String::new(),
            "blake3:".to_string(),
            format!("../{hex}"),
            format!("blake3:../{hex}"),
            hex.to_string(),
            uppercase,
            outside,
        ] {
            let (code, _) = blueprint_library_resolve_from_args(&dir.root_str(), &bad)
                .expect_err("invalid hash");
            assert_eq!(code, "invalid_blueprint_library_hash", "hash {bad:?}");
        }
    }

    #[test]
    fn blueprint_library_resolve_missing_hash_is_ok() {
        let dir = TempBlueprintLibrary::new("absent-hash");
        dir.create_root();
        let package = dir.package("house.tsbp");
        let hash = write_bridge_package(&package, "local:bridge_house");
        let resolved =
            blueprint_library_resolve_from_args(&dir.root_str(), &hash).expect("valid hash");
        assert!(resolved.is_none());
    }

    #[test]
    fn blueprint_library_collision_is_reported() {
        let dir = TempBlueprintLibrary::new("collision");
        dir.create_root();
        let first = dir.package("first.tsbp");
        let second = dir.package("second.tsbp");
        write_bridge_package(&first, "local:first");
        write_bridge_package(&second, "local:second");
        let (insert, _) =
            blueprint_library_insert_from_args(&dir.root_str(), &first.to_string_lossy())
                .expect("insert");
        std::fs::copy(&second, &insert.path).expect("different bytes");
        let (code, _) =
            blueprint_library_insert_from_args(&dir.root_str(), &first.to_string_lossy())
                .expect_err("collision");
        assert_eq!(code, "blueprint_library_collision");
    }

    #[test]
    fn blueprint_library_summary_is_stable_across_insertion_order() {
        let first = TempBlueprintLibrary::new("order-a");
        let second = TempBlueprintLibrary::new("order-b");
        first.create_root();
        second.create_root();
        let package_a = first.package("a.tsbp");
        let package_b = first.package("b.tsbp");
        write_bridge_package(&package_a, "local:a");
        write_bridge_package(&package_b, "local:b");

        blueprint_library_insert_from_args(&first.root_str(), &package_a.to_string_lossy())
            .expect("a first");
        blueprint_library_insert_from_args(&first.root_str(), &package_b.to_string_lossy())
            .expect("b second");
        blueprint_library_insert_from_args(&second.root_str(), &package_b.to_string_lossy())
            .expect("b first");
        blueprint_library_insert_from_args(&second.root_str(), &package_a.to_string_lossy())
            .expect("a second");

        let summary_a = blueprint_library_summary_from_arg(&first.root_str()).expect("a");
        let summary_b = blueprint_library_summary_from_arg(&second.root_str()).expect("b");
        assert_eq!(summary_a, summary_b);
        assert_eq!(summary_a.count, 2);
    }

    fn write_world_package(path: &Path, id: &str, name: &str, fill: u16) -> String {
        let mut volume =
            VoxelVolume::new(VolumeConfig::new(1, 1, 1).expect("config")).expect("volume");
        volume
            .fill_box(VolumePos::new(0, 0, 0), VolumePos::new(4, 1, 4), fill)
            .expect("fill floor");
        let blueprint = Blueprint::new(BlueprintMetadata::new(id, name), volume);
        save_tsbp_with_hash(path, &blueprint).expect("save package")
    }

    fn place_world_house(
        store: &mut PlacementStore,
        path: &Path,
        hash: &str,
        id: &str,
        name: &str,
        x: f32,
    ) -> Placement {
        let request = PlacementRequest {
            blueprint_id: id.to_string(),
            blueprint_hash: hash.to_string(),
            blueprint_path: path.to_string_lossy().into_owned(),
            name: name.to_string(),
            bounds_m: [3.2, 3.2, 3.2],
            center_x: x,
            center_z: 100.0,
            rotation_deg: 0,
            material_cost: 100.0,
            resident_capacity: 0,
            job_slots: 0,
            solid_voxels: 16,
            structure: None,
        };
        store.place(&request, |_, _| 40).expect("placement")
    }

    #[test]
    fn world_save_library_copies_unique_packages_with_counts() {
        let dir = TempBlueprintLibrary::new("world-save-copy");
        dir.create_root();
        let first = dir.package("first.tsbp");
        let second = dir.package("second.tsbp");
        let first_hash =
            write_world_package(&first, "local:first", "World Library House", EDITOR_FLOOR);
        let second_hash =
            write_world_package(&second, "local:second", "World Library House", EDITOR_WALL);
        let mut store = PlacementStore::new(2026);
        place_world_house(
            &mut store,
            &first,
            &first_hash,
            "local:first",
            "World Library House",
            100.0,
        );
        place_world_house(
            &mut store,
            &first,
            &first_hash,
            "local:first",
            "World Library House",
            200.0,
        );
        place_world_house(
            &mut store,
            &second,
            &second_hash,
            "local:second",
            "World Library House",
            300.0,
        );
        let before = store.to_json_pretty().expect("save json");
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        let sync = sync_world_blueprints(&store, &library).expect("sync");
        assert_eq!(sync.unique, 2);
        assert_eq!(sync.inserted, 2);
        assert_eq!(sync.existing, 0);
        assert_eq!(sync.library_count, 2);
        assert_eq!(library.summary().expect("summary").count, 2);
        assert_eq!(store.to_json_pretty().expect("save json"), before);
    }

    #[test]
    fn world_save_library_keeps_the_default_save_json_byte_identical() {
        let dir = TempBlueprintLibrary::new("world-save-identical");
        dir.create_root();
        let package = dir.package("house.tsbp");
        let hash =
            write_world_package(&package, "local:house", "World Library House", EDITOR_FLOOR);
        let mut store = PlacementStore::new(2026);
        place_world_house(
            &mut store,
            &package,
            &hash,
            "local:house",
            "World Library House",
            100.0,
        );
        let before_json = store.to_json_pretty().expect("save json");
        let before_checksum = store.checksum();
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        sync_world_blueprints(&store, &library).expect("sync");
        assert_eq!(store.to_json_pretty().expect("save json"), before_json);
        assert_eq!(store.checksum(), before_checksum);
    }

    #[test]
    fn world_save_library_repeat_save_reuses_existing_packages() {
        let dir = TempBlueprintLibrary::new("world-save-repeat");
        dir.create_root();
        let package = dir.package("house.tsbp");
        let hash =
            write_world_package(&package, "local:house", "World Library House", EDITOR_FLOOR);
        let mut store = PlacementStore::new(2026);
        place_world_house(
            &mut store,
            &package,
            &hash,
            "local:house",
            "World Library House",
            100.0,
        );
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        let first = sync_world_blueprints(&store, &library).expect("first sync");
        assert_eq!(first.inserted, 1);
        assert_eq!(first.existing, 0);
        let summary = library.summary().expect("summary");
        let second = sync_world_blueprints(&store, &library).expect("second sync");
        assert_eq!(second.inserted, 0);
        assert_eq!(second.existing, 1);
        assert_eq!(second.library_count, 1);
        assert_eq!(library.summary().expect("summary"), summary);
    }

    #[test]
    fn world_save_library_keeps_an_empty_world_unchanged() {
        let dir = TempBlueprintLibrary::new("world-save-empty");
        let root = dir.create_root();
        let store = PlacementStore::new(2026);
        let before = store.to_json_pretty().expect("save json");
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        let sync = sync_world_blueprints(&store, &library).expect("sync");
        assert_eq!(sync.unique, 0);
        assert_eq!(sync.inserted, 0);
        assert_eq!(sync.existing, 0);
        assert_eq!(sync.library_count, 0);
        assert!(std::fs::read_dir(&root).expect("root").next().is_none());
        assert_eq!(store.to_json_pretty().expect("save json"), before);
    }

    #[test]
    fn world_load_library_resolves_a_deleted_package() {
        let dir = TempBlueprintLibrary::new("world-load-deleted");
        dir.create_root();
        let package = dir.package("house.tsbp");
        let hash =
            write_world_package(&package, "local:house", "World Library House", EDITOR_FLOOR);
        let mut store = PlacementStore::new(2026);
        let placement = place_world_house(
            &mut store,
            &package,
            &hash,
            "local:house",
            "World Library House",
            100.0,
        );
        let before = store.to_json_pretty().expect("save json");
        let checksum = store.checksum();
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        sync_world_blueprints(&store, &library).expect("sync");
        std::fs::remove_file(&package).expect("remove source");

        let (loaded, resolved_path, source) =
            load_world_placement_package(&placement, Some(&library)).expect("library load");
        assert_eq!(source, PlacementPackageSource::LibraryHash);
        assert_eq!(loaded.metadata.content_hash, placement.blueprint_hash);
        assert_eq!(loaded.metadata.solid_voxels, 16);
        assert_eq!(
            resolved_path.as_deref(),
            library.resolve(&placement.blueprint_hash).as_deref()
        );
        let LoadedWorldPlacements {
            solid_by_id,
            resolved_paths,
            ..
        } = load_world_placement_packages(&store, Some(&library)).expect("library load all");
        assert_eq!(solid_by_id.get(&placement.id), Some(&16));
        assert_eq!(
            resolved_paths.get(&placement.blueprint_hash),
            Some(
                &library
                    .resolve(&placement.blueprint_hash)
                    .expect("resolved")
                    .to_string_lossy()
                    .into_owned()
            )
        );
        assert_eq!(store.checksum(), checksum);
        assert_eq!(store.to_json_pretty().expect("save json"), before);
    }

    #[test]
    fn world_load_without_library_keeps_the_original_missing_code() {
        let dir = TempBlueprintLibrary::new("world-load-no-library");
        dir.create_root();
        let package = dir.package("house.tsbp");
        let hash =
            write_world_package(&package, "local:house", "World Library House", EDITOR_FLOOR);
        let mut store = PlacementStore::new(2026);
        let placement = place_world_house(
            &mut store,
            &package,
            &hash,
            "local:house",
            "World Library House",
            100.0,
        );
        std::fs::remove_file(&package).expect("remove source");
        let (code, message) =
            load_world_placement_package(&placement, None).expect_err("missing package");
        assert_eq!(code, "missing_blueprint");
        assert!(message.contains("house.tsbp"), "{message}");
    }

    #[test]
    fn world_load_library_reports_an_unresolved_reference() {
        let dir = TempBlueprintLibrary::new("world-load-unresolved");
        dir.create_root();
        let package = dir.package("house.tsbp");
        let hash =
            write_world_package(&package, "local:house", "World Library House", EDITOR_FLOOR);
        let mut store = PlacementStore::new(2026);
        let placement = place_world_house(
            &mut store,
            &package,
            &hash,
            "local:house",
            "World Library House",
            100.0,
        );
        std::fs::remove_file(&package).expect("remove source");
        let before = store.to_json_pretty().expect("save json");
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        let (code, message) = load_world_placement_package(&placement, Some(&library))
            .expect_err("unresolved reference");
        assert_eq!(code, "blueprint_library_missing_file");
        assert!(message.contains(&hash), "{message}");
        assert_eq!(store.to_json_pretty().expect("save json"), before);
    }

    #[test]
    fn world_library_missing_root_reports_a_stable_code() {
        let dir = TempBlueprintLibrary::new("world-library-missing-root");
        assert!(!dir.root().exists());
        let (code, _) = blueprint_library_open(&dir.root_str()).expect_err("missing root");
        assert_eq!(code, "blueprint_library_root_missing");
    }

    #[test]
    fn world_library_invalid_root_reports_a_stable_code() {
        let (code, message) =
            blueprint_library_open("relative/library").expect_err("relative root");
        assert_eq!(code, "invalid_blueprint_library_root");
        assert!(message.contains("absolute"), "{message}");
        let traversal = std::env::temp_dir().join("one").join("..").join("two");
        let (code, _) =
            blueprint_library_open(&traversal.to_string_lossy()).expect_err("traversal root");
        assert_eq!(code, "invalid_blueprint_library_root");
    }

    #[test]
    fn world_save_library_rejects_a_missing_package_without_writing() {
        let dir = TempBlueprintLibrary::new("world-save-missing-package");
        dir.create_root();
        let package = dir.package("house.tsbp");
        let hash =
            write_world_package(&package, "local:house", "World Library House", EDITOR_FLOOR);
        let mut store = PlacementStore::new(2026);
        place_world_house(
            &mut store,
            &package,
            &hash,
            "local:house",
            "World Library House",
            100.0,
        );
        let before = store.to_json_pretty().expect("save json");
        std::fs::remove_file(&package).expect("remove source");
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        let (code, _) = sync_world_blueprints(&store, &library).expect_err("missing package");
        assert_eq!(code, "blueprint_library_missing_file");
        assert_eq!(library.summary().expect("summary").count, 0);
        assert_eq!(store.to_json_pretty().expect("save json"), before);
    }

    #[test]
    fn world_save_library_rejects_a_hash_mismatch_without_copying() {
        let dir = TempBlueprintLibrary::new("world-save-hash");
        dir.create_root();
        let package = dir.package("house.tsbp");
        write_world_package(&package, "local:house", "World Library House", EDITOR_FLOOR);
        let recorded_hash = format!("blake3:{}", "0".repeat(64));
        let mut store = PlacementStore::new(2026);
        place_world_house(
            &mut store,
            &package,
            &recorded_hash,
            "local:house",
            "World Library House",
            100.0,
        );
        let before = store.to_json_pretty().expect("save json");
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        let (code, message) = sync_world_blueprints(&store, &library).expect_err("hash mismatch");
        assert_eq!(code, "blueprint_hash_mismatch");
        assert!(message.contains(&recorded_hash), "{message}");
        assert_eq!(library.summary().expect("summary").count, 0);
        assert_eq!(store.to_json_pretty().expect("save json"), before);
    }

    #[test]
    fn world_save_library_rejects_a_metadata_mismatch_without_copying() {
        let dir = TempBlueprintLibrary::new("world-save-metadata");
        dir.create_root();
        let package = dir.package("house.tsbp");
        let hash = write_world_package(
            &package,
            "local:actual",
            "World Library House",
            EDITOR_FLOOR,
        );
        let mut store = PlacementStore::new(2026);
        place_world_house(
            &mut store,
            &package,
            &hash,
            "local:recorded",
            "World Library House",
            100.0,
        );
        let before = store.to_json_pretty().expect("save json");
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        let (code, _) = sync_world_blueprints(&store, &library).expect_err("metadata mismatch");
        assert_eq!(code, "blueprint_metadata_mismatch");
        assert_eq!(library.summary().expect("summary").count, 0);
        assert_eq!(store.to_json_pretty().expect("save json"), before);
    }

    #[test]
    fn world_save_library_rejects_a_corrupt_package_without_copying() {
        let dir = TempBlueprintLibrary::new("world-save-corrupt");
        dir.create_root();
        let package = dir.package("house.tsbp");
        let hash =
            write_world_package(&package, "local:house", "World Library House", EDITOR_FLOOR);
        let mut store = PlacementStore::new(2026);
        place_world_house(
            &mut store,
            &package,
            &hash,
            "local:house",
            "World Library House",
            100.0,
        );
        let before = store.to_json_pretty().expect("save json");
        std::fs::write(&package, b"not a zip").expect("corrupt source");
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        let (code, _) = sync_world_blueprints(&store, &library).expect_err("corrupt package");
        assert_eq!(code, "blueprint_library_invalid_package");
        assert_eq!(library.summary().expect("summary").count, 0);
        assert_eq!(store.to_json_pretty().expect("save json"), before);
    }

    #[test]
    fn world_load_library_rejects_a_corrupt_stored_package() {
        let dir = TempBlueprintLibrary::new("world-load-corrupt");
        let root = dir.create_root();
        let package = dir.package("house.tsbp");
        let hash =
            write_world_package(&package, "local:house", "World Library House", EDITOR_FLOOR);
        let mut store = PlacementStore::new(2026);
        let placement = place_world_house(
            &mut store,
            &package,
            &hash,
            "local:house",
            "World Library House",
            100.0,
        );
        std::fs::remove_file(&package).expect("remove source");
        let hex = &hash["blake3:".len()..];
        let shard = root.join(&hex[..2]);
        std::fs::create_dir_all(&shard).expect("shard");
        std::fs::write(shard.join(format!("{hex}.tsbp")), b"not a zip").expect("corrupt entry");
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        let (code, _) = load_world_placement_package(&placement, Some(&library))
            .expect_err("corrupt library entry");
        assert_eq!(code, "blueprint_library_invalid_package");
    }

    #[test]
    fn world_load_library_rejects_a_foreign_stored_package() {
        let dir = TempBlueprintLibrary::new("world-load-foreign");
        let root = dir.create_root();
        let first = dir.package("first.tsbp");
        let second = dir.package("second.tsbp");
        let first_hash =
            write_world_package(&first, "local:first", "World Library House", EDITOR_FLOOR);
        write_world_package(&second, "local:second", "World Library House", EDITOR_WALL);
        let mut store = PlacementStore::new(2026);
        let placement = place_world_house(
            &mut store,
            &first,
            &first_hash,
            "local:first",
            "World Library House",
            100.0,
        );
        std::fs::remove_file(&first).expect("remove source");
        let hex = &first_hash["blake3:".len()..];
        let shard = root.join(&hex[..2]);
        std::fs::create_dir_all(&shard).expect("shard");
        std::fs::copy(&second, shard.join(format!("{hex}.tsbp"))).expect("foreign entry");
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        let (code, _) =
            load_world_placement_package(&placement, Some(&library)).expect_err("foreign package");
        assert_eq!(code, "blueprint_hash_mismatch");
    }

    #[test]
    fn world_load_library_rejects_a_metadata_mismatch() {
        let dir = TempBlueprintLibrary::new("world-load-metadata");
        dir.create_root();
        let package = dir.package("house.tsbp");
        let hash =
            write_world_package(&package, "local:house", "World Library House", EDITOR_FLOOR);
        let mut store = PlacementStore::new(2026);
        let placement = place_world_house(
            &mut store,
            &package,
            &hash,
            "local:house",
            "Recorded House",
            100.0,
        );
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        library.insert_file(&package).expect("insert package");
        std::fs::remove_file(&package).expect("remove source");
        let (code, _) = load_world_placement_package(&placement, Some(&library))
            .expect_err("metadata mismatch");
        assert_eq!(code, "blueprint_metadata_mismatch");
    }

    #[test]
    fn world_save_library_keeps_the_store_seed_and_checksum() {
        let dir = TempBlueprintLibrary::new("world-save-seed");
        dir.create_root();
        let package = dir.package("house.tsbp");
        let hash =
            write_world_package(&package, "local:house", "World Library House", EDITOR_FLOOR);
        let mut store = PlacementStore::new(2027);
        place_world_house(
            &mut store,
            &package,
            &hash,
            "local:house",
            "World Library House",
            100.0,
        );
        let before_checksum = store.checksum();
        let before_json = store.to_json_pretty().expect("save json");
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        sync_world_blueprints(&store, &library).expect("sync");
        assert_eq!(store.seed, 2027);
        assert_eq!(store.checksum(), before_checksum);
        assert_eq!(store.to_json_pretty().expect("save json"), before_json);
    }

    fn world_library_package(
        dir: &TempBlueprintLibrary,
        name: &str,
        id: &str,
        label: &str,
    ) -> (PathBuf, String) {
        let package = dir.package(name);
        let hash = write_world_package(&package, id, label, EDITOR_FLOOR);
        (package, hash)
    }

    fn library_resolved_path(dir: &TempBlueprintLibrary, hash: &str) -> String {
        BlueprintLibrary::new(dir.root())
            .expect("library")
            .resolve(hash)
            .expect("resolved entry")
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn world_load_library_exposes_the_resolved_path_to_placements() {
        let dir = TempBlueprintLibrary::new("world-render-resolved");
        dir.create_root();
        let (package, hash) = world_library_package(
            &dir,
            "unique_library_house.tsbp",
            "local:library_house",
            "Library House",
        );
        let mut store = PlacementStore::new(2026);
        let placed = place_world_house(
            &mut store,
            &package,
            &hash,
            "local:library_house",
            "Library House",
            100.0,
        );
        let before_checksum = store.checksum();
        let before_json = store.to_json_pretty().expect("save json");
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        sync_world_blueprints(&store, &library).expect("save with library");
        std::fs::remove_file(&package).expect("remove source package");

        let LoadedWorldPlacements {
            solid_by_id,
            resolved_paths: paths,
            ..
        } = load_world_placement_packages(&store, Some(&library)).expect("portable load");
        assert_eq!(solid_by_id.get(&placed.id), Some(&placed.solid_voxels));
        let mut resolved = ResolvedBlueprintPaths::default();
        resolved.replace(paths);
        assert_eq!(resolved.paths.len(), 1);
        assert_eq!(
            resolved.path_for(&placed),
            Some(library_resolved_path(&dir, &hash).as_str())
        );
        assert_eq!(store.placements[0].blueprint_path, placed.blueprint_path);
        assert_eq!(store.checksum(), before_checksum);
        assert_eq!(store.to_json_pretty().expect("save json"), before_json);
    }

    #[test]
    fn world_load_without_library_has_no_resolved_path() {
        let dir = TempBlueprintLibrary::new("world-render-plain");
        dir.create_root();
        let (package, hash) =
            world_library_package(&dir, "plain_house.tsbp", "local:plain_house", "Plain House");
        let mut store = PlacementStore::new(2026);
        place_world_house(
            &mut store,
            &package,
            &hash,
            "local:plain_house",
            "Plain House",
            100.0,
        );
        let before_json = store.to_json_pretty().expect("save json");
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        sync_world_blueprints(&store, &library).expect("save with library");
        std::fs::remove_file(&package).expect("remove source package");

        let (code, message) = load_world_placement_packages(&store, None).expect_err("plain load");
        assert_eq!(code, "missing_blueprint");
        assert!(message.contains("plain_house.tsbp"), "{message}");
        let resolved = ResolvedBlueprintPaths::default();
        assert_eq!(resolved.path_for(&store.placements[0]), None);
        assert_eq!(store.to_json_pretty().expect("save json"), before_json);
    }

    #[test]
    fn failed_library_load_keeps_the_previous_resolved_map() {
        let dir = TempBlueprintLibrary::new("world-render-failed");
        dir.create_root();
        let (package, hash) = world_library_package(
            &dir,
            "failed_house.tsbp",
            "local:failed_house",
            "Failed House",
        );
        let mut store = PlacementStore::new(2026);
        place_world_house(
            &mut store,
            &package,
            &hash,
            "local:failed_house",
            "Failed House",
            100.0,
        );
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        sync_world_blueprints(&store, &library).expect("save with library");
        std::fs::remove_file(&package).expect("remove source package");
        let LoadedWorldPlacements {
            resolved_paths: paths,
            ..
        } = load_world_placement_packages(&store, Some(&library)).expect("portable load");
        let mut resolved = ResolvedBlueprintPaths::default();
        resolved.replace(paths);
        let before = resolved.path_for(&store.placements[0]).map(str::to_owned);
        let checksum = store.checksum();

        let entry = library.resolve(&hash).expect("library entry");
        std::fs::remove_file(&entry).expect("remove library entry");
        // A stale entry is reported as-is, not silently dropped.
        assert_eq!(resolved.path_for(&store.placements[0]), before.as_deref());

        let (code, _) =
            load_world_placement_packages(&store, Some(&library)).expect_err("missing entry");
        assert_eq!(code, "blueprint_library_missing_file");
        assert_eq!(resolved.paths.len(), 1);
        assert_eq!(resolved.path_for(&store.placements[0]), before.as_deref());
        assert_eq!(store.checksum(), checksum);
    }

    #[test]
    fn shared_library_hash_resolves_once_and_removal_prunes_the_map() {
        let dir = TempBlueprintLibrary::new("world-render-shared");
        dir.create_root();
        let (package, hash) = world_library_package(
            &dir,
            "shared_house.tsbp",
            "local:shared_house",
            "Shared House",
        );
        let mut store = PlacementStore::new(2026);
        let first = place_world_house(
            &mut store,
            &package,
            &hash,
            "local:shared_house",
            "Shared House",
            100.0,
        );
        let second = place_world_house(
            &mut store,
            &package,
            &hash,
            "local:shared_house",
            "Shared House",
            200.0,
        );
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        sync_world_blueprints(&store, &library).expect("save with library");
        std::fs::remove_file(&package).expect("remove source package");
        let LoadedWorldPlacements {
            resolved_paths: paths,
            ..
        } = load_world_placement_packages(&store, Some(&library)).expect("portable load");
        let mut resolved = ResolvedBlueprintPaths::default();
        resolved.replace(paths);
        assert_eq!(resolved.paths.len(), 1);
        assert_eq!(resolved.path_for(&first), resolved.path_for(&second));

        assert!(store.remove(first.id));
        resolved.prune(&store);
        assert_eq!(resolved.paths.len(), 1);
        assert!(store.remove(second.id));
        resolved.prune(&store);
        assert!(resolved.paths.is_empty());
    }

    #[test]
    fn a_valid_recorded_path_has_no_resolved_override() {
        let dir = TempBlueprintLibrary::new("world-render-valid");
        dir.create_root();
        let (package, hash) =
            world_library_package(&dir, "valid_house.tsbp", "local:valid_house", "Valid House");
        let mut store = PlacementStore::new(2026);
        place_world_house(
            &mut store,
            &package,
            &hash,
            "local:valid_house",
            "Valid House",
            100.0,
        );
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        sync_world_blueprints(&store, &library).expect("save with library");
        let LoadedWorldPlacements {
            resolved_paths: paths,
            ..
        } = load_world_placement_packages(&store, Some(&library)).expect("portable load");
        let mut resolved = ResolvedBlueprintPaths::default();
        resolved.replace(paths);
        assert!(resolved.paths.is_empty());
        assert_eq!(resolved.path_for(&store.placements[0]), None);
    }

    #[test]
    fn plain_load_after_a_portable_load_clears_the_map() {
        let dir = TempBlueprintLibrary::new("world-render-plain-clear");
        dir.create_root();
        let (package, hash) = world_library_package(
            &dir,
            "plain_clear_house.tsbp",
            "local:plain_clear_house",
            "Plain Clear House",
        );
        let mut store = PlacementStore::new(2026);
        place_world_house(
            &mut store,
            &package,
            &hash,
            "local:plain_clear_house",
            "Plain Clear House",
            100.0,
        );
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        sync_world_blueprints(&store, &library).expect("save with library");
        std::fs::remove_file(&package).expect("remove source package");
        let LoadedWorldPlacements {
            resolved_paths: paths,
            ..
        } = load_world_placement_packages(&store, Some(&library)).expect("portable load");
        let mut resolved = ResolvedBlueprintPaths::default();
        resolved.replace(paths);
        assert_eq!(resolved.paths.len(), 1);

        let entry = library.resolve(&hash).expect("library entry");
        std::fs::copy(entry, &package).expect("restore source package");
        let LoadedWorldPlacements {
            resolved_paths: plain_paths,
            ..
        } = load_world_placement_packages(&store, None).expect("plain load");
        resolved.replace(plain_paths);
        assert!(resolved.paths.is_empty());
        assert_eq!(resolved.path_for(&store.placements[0]), None);
    }

    #[test]
    fn world_clear_placements_clears_the_resolved_map() {
        let dir = TempBlueprintLibrary::new("world-render-clear");
        dir.create_root();
        let (package, hash) =
            world_library_package(&dir, "clear_house.tsbp", "local:clear_house", "Clear House");
        let mut store = PlacementStore::new(2026);
        place_world_house(
            &mut store,
            &package,
            &hash,
            "local:clear_house",
            "Clear House",
            100.0,
        );
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        sync_world_blueprints(&store, &library).expect("save with library");
        std::fs::remove_file(&package).expect("remove source package");
        let LoadedWorldPlacements {
            resolved_paths: paths,
            ..
        } = load_world_placement_packages(&store, Some(&library)).expect("portable load");
        let mut resolved = ResolvedBlueprintPaths::default();
        resolved.replace(paths);
        assert_eq!(resolved.paths.len(), 1);

        resolved.clear();
        assert!(resolved.paths.is_empty());
        assert_eq!(resolved.path_for(&store.placements[0]), None);
    }

    fn bridge_fixture(dir: &TempBlueprintLibrary) -> (PathBuf, String, PlacementStore) {
        dir.create_root();
        let (package, hash) =
            world_library_package(dir, "house.tsbp", "local:house", "World Library House");
        let mut store = PlacementStore::new(2026);
        place_world_house(
            &mut store,
            &package,
            &hash,
            "local:house",
            "World Library House",
            100.0,
        );
        (package, hash, store)
    }

    fn bridge_json(json: &str) -> serde_json::Value {
        serde_json::from_str(json).expect("world json")
    }

    fn collect_json_strings<'a>(value: &'a serde_json::Value, strings: &mut Vec<&'a str>) {
        match value {
            serde_json::Value::String(text) => strings.push(text),
            serde_json::Value::Array(items) => {
                for item in items {
                    collect_json_strings(item, strings);
                }
            }
            serde_json::Value::Object(fields) => {
                for item in fields.values() {
                    collect_json_strings(item, strings);
                }
            }
            serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
            }
        }
    }

    /// Compare parsed strings, not raw JSON: Windows paths contain backslashes
    /// that serde escapes inside the serialized document.
    fn assert_no_library_root(json: &str, roots: &[&Path]) {
        let value = bridge_json(json);
        let mut strings = Vec::new();
        collect_json_strings(&value, &mut strings);
        for root in roots {
            let root = root.to_string_lossy();
            assert!(
                strings.iter().all(|text| !text.contains(root.as_ref())),
                "the library root {root} must not enter the save"
            );
        }
    }

    #[test]
    fn stamping_adds_only_a_content_address_and_keeps_the_legacy_path() {
        let dir = TempBlueprintLibrary::new("bridge-refs-stamp");
        let (package, hash, mut store) = bridge_fixture(&dir);
        let before_checksum = store.checksum();
        let recorded = store.placements[0].blueprint_path.clone();
        let json_before = store.to_json_pretty().expect("legacy json");
        assert!(!json_before.contains("blueprint_ref"));

        stamp_world_blueprint_references(&mut store);
        let json = store.to_json_pretty().expect("stamped json");
        let value = bridge_json(&json);
        let reference = format!("tsbp:{hash}");
        assert_eq!(
            value["placements"][0]["blueprint_ref"].as_str(),
            Some(reference.as_str())
        );
        assert_eq!(
            value["placements"][0]["blueprint_path"].as_str(),
            Some(recorded.as_str()),
            "the legacy path stays as a fallback"
        );
        assert_no_library_root(&json, &[dir.root().as_path()]);
        assert_ne!(store.checksum(), before_checksum);
        assert_eq!(store.placements[0].blueprint_path, recorded);
        let reloaded = PlacementStore::from_json(&json).expect("stamped save loads");
        assert_eq!(
            reloaded.placements[0].blueprint_ref.as_deref(),
            Some(format!("tsbp:{hash}").as_str())
        );
        assert_eq!(
            reloaded.to_json_pretty().expect("roundtrip json"),
            json,
            "a later plain save preserves the reference bytes"
        );
        assert!(package.exists());
    }

    #[test]
    fn sync_accepts_a_missing_path_when_the_reference_resolves() {
        let dir = TempBlueprintLibrary::new("bridge-refs-sync-missing");
        let (package, hash, mut store) = bridge_fixture(&dir);
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        library.insert_file(&package).expect("insert");
        stamp_world_blueprint_references(&mut store);
        let json_before = store.to_json_pretty().expect("json");
        std::fs::remove_file(&package).expect("remove source");

        let sync = sync_world_blueprints(&store, &library).expect("reference save");
        assert_eq!(sync.unique, 1);
        assert_eq!(sync.inserted, 0);
        assert_eq!(sync.existing, 1);
        assert_eq!(sync.library_count, 1);
        assert_eq!(store.to_json_pretty().expect("json"), json_before);
        assert_eq!(
            store.placements[0].blueprint_ref.as_deref(),
            Some(format!("tsbp:{hash}").as_str())
        );
        assert_no_library_root(&json_before, &[dir.root().as_path()]);
    }

    #[test]
    fn sync_without_a_reference_still_rejects_a_missing_path() {
        let dir = TempBlueprintLibrary::new("bridge-refs-sync-legacy-missing");
        let (package, _hash, store) = bridge_fixture(&dir);
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        std::fs::remove_file(&package).expect("remove source");
        let (code, _) = sync_world_blueprints(&store, &library).expect_err("missing source");
        assert_eq!(code, "blueprint_library_missing_file");
        assert_eq!(library.summary().expect("summary").count, 0);
    }

    #[test]
    fn sync_rejects_a_mismatched_reference_before_copying() {
        let dir = TempBlueprintLibrary::new("bridge-refs-sync-mismatch");
        let (_package, _hash, mut store) = bridge_fixture(&dir);
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        stamp_world_blueprint_references(&mut store);
        store.placements[0].blueprint_ref = Some(format!("tsbp:blake3:{}", "0".repeat(64)));
        let json_before = store.to_json_pretty().expect("json");
        let (code, _) = sync_world_blueprints(&store, &library).expect_err("mismatch");
        assert_eq!(code, "blueprint_hash_mismatch");
        assert_eq!(library.summary().expect("summary").count, 0);
        assert_eq!(store.to_json_pretty().expect("json"), json_before);
    }

    #[test]
    fn load_resolves_the_reference_before_the_recorded_path() {
        let dir = TempBlueprintLibrary::new("bridge-refs-load-order");
        let (package, hash, mut store) = bridge_fixture(&dir);
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        library.insert_file(&package).expect("insert");
        stamp_world_blueprint_references(&mut store);
        let placement = store.placements[0].clone();
        let json_before = store.to_json_pretty().expect("json");

        let (loaded, resolved, source) =
            load_world_placement_package(&placement, Some(&library)).expect("reference load");
        assert_eq!(source, PlacementPackageSource::Reference);
        assert_eq!(loaded.metadata.content_hash, hash);
        assert_eq!(
            resolved.as_deref(),
            library.resolve(&hash).as_deref(),
            "the reference is preferred even when the recorded path still works"
        );
        assert_eq!(store.to_json_pretty().expect("json"), json_before);
    }

    #[test]
    fn load_without_a_library_uses_a_working_legacy_path() {
        let dir = TempBlueprintLibrary::new("bridge-refs-load-legacy-path");
        let (_package, _hash, mut store) = bridge_fixture(&dir);
        stamp_world_blueprint_references(&mut store);
        let placement = store.placements[0].clone();

        let (loaded, resolved, source) =
            load_world_placement_package(&placement, None).expect("legacy path load");
        assert_eq!(source, PlacementPackageSource::LegacyPathFallback);
        assert_eq!(loaded.metadata.id, "local:house");
        assert!(resolved.is_none());
    }

    #[test]
    fn load_without_a_library_and_a_missing_path_reports_the_reference_error() {
        let dir = TempBlueprintLibrary::new("bridge-refs-load-no-library");
        let (package, _hash, mut store) = bridge_fixture(&dir);
        stamp_world_blueprint_references(&mut store);
        let placement = store.placements[0].clone();
        std::fs::remove_file(&package).expect("remove source");

        let (code, message) =
            load_world_placement_package(&placement, None).expect_err("no library");
        assert_eq!(code, "blueprint_reference_requires_library");
        assert!(
            message.contains("requires a blueprint library"),
            "{message}"
        );
        assert!(!message.contains("tsbp:blake3:"));
    }

    #[test]
    fn load_rejects_a_malformed_or_mismatched_reference() {
        let dir = TempBlueprintLibrary::new("bridge-refs-load-shape");
        let (_package, hash, store) = bridge_fixture(&dir);
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        let mut placement = store.placements[0].clone();

        placement.blueprint_ref = Some("blake3:not-a-reference".to_string());
        let (code, _) =
            load_world_placement_package(&placement, Some(&library)).expect_err("shape");
        assert_eq!(code, "invalid_blueprint_reference");

        placement.blueprint_ref = Some(format!("tsbp:blake3:{}", "0".repeat(64)));
        let (code, message) =
            load_world_placement_package(&placement, Some(&library)).expect_err("mismatch");
        assert_eq!(code, "blueprint_hash_mismatch");
        assert!(message.contains(&hash), "{message}");
        assert!(store.placements[0].blueprint_ref.is_none());
    }

    #[test]
    fn load_fails_closed_when_the_reference_entry_is_missing() {
        let dir = TempBlueprintLibrary::new("bridge-refs-load-missing-entry");
        let (package, _hash, mut store) = bridge_fixture(&dir);
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        stamp_world_blueprint_references(&mut store);
        let placement = store.placements[0].clone();

        let (code, _) =
            load_world_placement_package(&placement, Some(&library)).expect_err("missing entry");
        assert_eq!(code, "blueprint_library_missing_file");
        // The recorded path is still valid; the reference failure must not fall
        // back to it.
        assert!(package.exists());
        assert!(load_world_placement_package(&placement, None).is_ok());
    }

    #[test]
    fn load_fails_closed_when_the_reference_entry_is_invalid() {
        let dir = TempBlueprintLibrary::new("bridge-refs-load-invalid-entry");
        let (package, hash, mut store) = bridge_fixture(&dir);
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        library.insert_file(&package).expect("insert");
        stamp_world_blueprint_references(&mut store);
        let placement = store.placements[0].clone();
        let entry = library.resolve(&hash).expect("entry");
        std::fs::write(&entry, b"not a zip").expect("corrupt entry");

        let (code, _) =
            load_world_placement_package(&placement, Some(&library)).expect_err("invalid entry");
        assert_eq!(code, "blueprint_library_invalid_package");
        assert!(package.exists());
        assert!(load_world_placement_package(&placement, None).is_ok());
    }

    #[test]
    fn load_rejects_a_reference_package_with_foreign_metadata() {
        let dir = TempBlueprintLibrary::new("bridge-refs-load-metadata");
        let (package, hash, mut store) = bridge_fixture(&dir);
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        library.insert_file(&package).expect("insert");
        stamp_world_blueprint_references(&mut store);
        store.placements[0].name = "Renamed House".to_string();
        let placement = store.placements[0].clone();
        assert_eq!(placement.blueprint_hash, hash);

        let (code, _) =
            load_world_placement_package(&placement, Some(&library)).expect_err("metadata");
        assert_eq!(code, "blueprint_metadata_mismatch");
    }

    #[test]
    fn legacy_records_keep_their_path_lookup_and_library_hash_fallback() {
        let dir = TempBlueprintLibrary::new("bridge-refs-load-plain");
        let (package, hash, store) = bridge_fixture(&dir);
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        library.insert_file(&package).expect("insert");
        let placement = store.placements[0].clone();

        let (_package, resolved, source) =
            load_world_placement_package(&placement, Some(&library)).expect("recorded path");
        assert_eq!(source, PlacementPackageSource::Recorded);
        assert!(resolved.is_none());

        std::fs::remove_file(&package).expect("remove source");
        let (loaded, resolved, source) =
            load_world_placement_package(&placement, Some(&library)).expect("hash fallback");
        assert_eq!(source, PlacementPackageSource::LibraryHash);
        assert_eq!(loaded.metadata.content_hash, hash);
        assert_eq!(resolved.as_deref(), library.resolve(&hash).as_deref());
    }

    #[test]
    fn load_counts_references_and_legacy_path_fallbacks() {
        let dir = TempBlueprintLibrary::new("bridge-refs-load-counts");
        let (package, hash, mut store) = bridge_fixture(&dir);
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        library.insert_file(&package).expect("insert");
        stamp_world_blueprint_references(&mut store);
        place_world_house(
            &mut store,
            &package,
            &hash,
            "local:house",
            "World Library House",
            200.0,
        );
        std::fs::remove_file(&package).expect("remove source");
        let only_reference = PlacementStore {
            format_version: store.format_version,
            seed: store.seed,
            next_id: store.placements[0].id + 1,
            placements: vec![store.placements[0].clone()],
        };
        let only_legacy_path = PlacementStore {
            format_version: store.format_version,
            seed: store.seed,
            next_id: store.placements[1].id + 1,
            placements: vec![store.placements[1].clone()],
        };

        let LoadedWorldPlacements {
            resolved_references,
            legacy_path_fallbacks,
            ..
        } = load_world_placement_packages(&only_reference, Some(&library)).expect("reference load");
        assert_eq!(resolved_references, 1);
        assert_eq!(legacy_path_fallbacks, 0);

        std::fs::copy(library.resolve(&hash).expect("entry"), &package).expect("restore source");
        let LoadedWorldPlacements {
            resolved_references,
            legacy_path_fallbacks,
            ..
        } = load_world_placement_packages(&only_legacy_path, None).expect("legacy path load");
        assert_eq!(resolved_references, 0);
        assert_eq!(legacy_path_fallbacks, 0);
    }

    #[test]
    fn load_counts_a_reference_loaded_from_its_legacy_path() {
        let dir = TempBlueprintLibrary::new("bridge-refs-load-fallback-count");
        let (_package, _hash, mut store) = bridge_fixture(&dir);
        stamp_world_blueprint_references(&mut store);
        let LoadedWorldPlacements {
            resolved_references,
            legacy_path_fallbacks,
            ..
        } = load_world_placement_packages(&store, None).expect("legacy path load");
        assert_eq!(resolved_references, 0);
        assert_eq!(legacy_path_fallbacks, 1);
    }

    #[test]
    fn two_libraries_with_the_same_bytes_yield_identical_stamped_saves() {
        let first = TempBlueprintLibrary::new("bridge-refs-identical-a");
        let second = TempBlueprintLibrary::new("bridge-refs-identical-b");
        let (package, hash, store) = bridge_fixture(&first);
        second.create_root();
        let first_library = BlueprintLibrary::new(first.root()).expect("first library");
        let second_library = BlueprintLibrary::new(second.root()).expect("second library");

        let mut first_saved = store.clone();
        stamp_world_blueprint_references(&mut first_saved);
        let first_sync = sync_world_blueprints(&first_saved, &first_library).expect("first sync");
        let mut second_saved = store;
        stamp_world_blueprint_references(&mut second_saved);
        let second_sync =
            sync_world_blueprints(&second_saved, &second_library).expect("second sync");

        assert_eq!(first_sync.library_count, second_sync.library_count);
        assert_eq!(first_saved.checksum(), second_saved.checksum());
        assert_eq!(
            first_saved.to_json_pretty().expect("first json"),
            second_saved.to_json_pretty().expect("second json")
        );
        assert_eq!(
            first_saved.placements[0].blueprint_ref.as_deref(),
            Some(format!("tsbp:{hash}").as_str())
        );
        let json = first_saved.to_json_pretty().expect("json");
        assert_no_library_root(&json, &[first.root().as_path(), second.root().as_path()]);
        assert_eq!(
            std::fs::read(first_library.resolve(&hash).expect("first entry")).expect("first bytes"),
            std::fs::read(second_library.resolve(&hash).expect("second entry"))
                .expect("second bytes")
        );
        assert!(package.exists());
    }

    #[test]
    fn a_plain_legacy_save_after_a_stamped_save_keeps_the_reference() {
        let dir = TempBlueprintLibrary::new("bridge-refs-plain-after");
        let (_package, hash, mut store) = bridge_fixture(&dir);
        let library = BlueprintLibrary::new(dir.root()).expect("library");
        stamp_world_blueprint_references(&mut store);
        sync_world_blueprints(&store, &library).expect("library save");
        let library_json = store.to_json_pretty().expect("library json");

        let plain_json = store.to_json_pretty().expect("plain json");
        assert_eq!(plain_json, library_json);
        let reference = format!("tsbp:{hash}");
        assert_eq!(
            bridge_json(&plain_json)["placements"][0]["blueprint_ref"].as_str(),
            Some(reference.as_str())
        );
        assert_eq!(
            PlacementStore::from_json(&plain_json)
                .expect("plain load")
                .placements[0]
                .blueprint_ref
                .as_deref(),
            Some(format!("tsbp:{hash}").as_str())
        );
    }

    #[test]
    fn placement_request_passes_active_ids_to_the_dependency_loader() {
        let dir = TempBlueprintLibrary::new("placement-deps");
        let path = dir.package("deps.tsbp");
        write_bridge_package_with_dependencies(&path, "townsmith:deps", &["mymod"], &[]);
        let path = path.to_string_lossy().into_owned();

        let (_, request) = world_placement_request(7, &path, 1.0, 1.0, 0, &["mymod:pack"])
            .expect("namespace match accepts mymod:pack");
        assert_eq!(request.blueprint_id, "townsmith:deps");
        let (_, request) = world_placement_request(7, &path, 1.0, 1.0, 0, &["mymod"])
            .expect("exact id accepts mymod");
        assert_eq!(request.blueprint_id, "townsmith:deps");
    }

    #[test]
    fn plain_world_json_for_a_legacy_placement_has_no_reference() {
        let request = PlacementRequest {
            blueprint_id: "local:house".to_string(),
            blueprint_hash: "blake3:test".to_string(),
            blueprint_path: "/townsmith/house.tsbp".to_string(),
            name: "House".to_string(),
            bounds_m: [8.0, 6.0, 6.0],
            center_x: 128.0,
            center_z: 128.0,
            rotation_deg: 0,
            material_cost: 100.0,
            resident_capacity: 2,
            job_slots: 0,
            solid_voxels: 100,
            structure: None,
        };
        let mut store = PlacementStore::new(2026);
        store.place(&request, |_, _| 32).expect("placement");
        let json = attach_construction(
            &store.to_json_pretty().expect("placement json"),
            &ConstructionStore::default(),
        )
        .expect("world json");
        assert!(!json.contains("blueprint_ref"));
        assert_eq!(store.checksum(), 0xc552_e58d_3d06_0904);
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in json.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        // Master `db5c495` output for this fixture; the plain path is
        // unchanged by the reference field.
        assert_eq!(hash, 0xba02_8161_4bbf_8304);
    }

    fn bridge_player_registry() -> PlayerRegistry {
        PlayerRegistry::new(BRIDGE_PLAYER_DEFAULT_SEED)
    }

    #[test]
    fn players_register_assigns_first_id_one_and_second_id_two() {
        let mut registry = bridge_player_registry();
        let first = players_register_in_store(&mut registry, "Ada").expect("first");
        assert_eq!(first.id.get(), 1);
        assert_eq!(first.display_name, "Ada");
        let second = players_register_in_store(&mut registry, "Grace").expect("second");
        assert_eq!(second.id.get(), 2);
        assert_eq!(second.display_name, "Grace");
        assert_eq!(registry.count(), 2);
    }

    #[test]
    fn players_register_reuses_a_case_insensitive_duplicate() {
        let mut registry = bridge_player_registry();
        let first = players_register_in_store(&mut registry, "Ada").expect("first");
        let before = registry.checksum();
        let again = players_register_in_store(&mut registry, "  aDA  ").expect("duplicate");
        assert_eq!(again.id, first.id);
        assert_eq!(again.display_name, "Ada", "the original casing is kept");
        assert_eq!(registry.count(), 1);
        assert_eq!(
            registry.checksum(),
            before,
            "an idempotent call mutates nothing"
        );
    }

    #[test]
    fn players_register_trims_names_and_accepts_the_32_char_limit() {
        let mut registry = bridge_player_registry();
        let name = "x".repeat(BRIDGE_PLAYER_NAME_CHARS);
        let player =
            players_register_in_store(&mut registry, &format!("  {name}  ")).expect("32 chars");
        assert_eq!(player.display_name, name);
    }

    #[test]
    fn players_register_rejects_empty_control_and_overlong_names() {
        let mut registry = bridge_player_registry();
        let overlong = "x".repeat(BRIDGE_PLAYER_NAME_CHARS + 1);
        for name in ["", "   ", "a\nb", "a\tb", overlong.as_str()] {
            let (code, _) = players_register_in_store(&mut registry, name).expect_err(name);
            assert_eq!(code, "player.invalid_name", "name {name:?}");
        }
        assert!(registry.is_empty());
        assert_eq!(registry.checksum(), PlayerRegistry::new(0).checksum());
    }

    #[test]
    fn players_register_reports_the_256_player_limit() {
        let mut registry = bridge_player_registry();
        for index in 0..core_city::player::MAX_PLAYERS {
            players_register_in_store(&mut registry, &format!("Player {index}"))
                .expect("under the limit");
        }
        assert_eq!(registry.count(), core_city::player::MAX_PLAYERS);
        let (code, _) =
            players_register_in_store(&mut registry, "Player 256").expect_err("full registry");
        assert_eq!(code, "player.limit");
        assert_eq!(registry.count(), core_city::player::MAX_PLAYERS);
        let existing = players_register_in_store(&mut registry, "player 0")
            .expect("a duplicate is still idempotent at the cap");
        assert_eq!(existing.id.get(), 1);
    }

    #[test]
    fn players_list_is_sorted_and_carries_the_registry_checksum() {
        let mut registry = bridge_player_registry();
        players_register_in_store(&mut registry, "Ada").expect("ada");
        players_register_in_store(&mut registry, "Grace").expect("grace");
        let snapshot = players_snapshot(&registry);
        assert_eq!(snapshot.count, 2);
        assert_eq!(
            snapshot.players,
            vec![
                BridgePlayerRecord {
                    id: 1,
                    name: "Ada".to_string(),
                },
                BridgePlayerRecord {
                    id: 2,
                    name: "Grace".to_string(),
                },
            ]
        );
        assert_eq!(snapshot.checksum, format!("{:016x}", registry.checksum()));
        assert_eq!(snapshot.checksum.len(), 16);
        assert!(snapshot
            .checksum
            .chars()
            .all(|value| value.is_ascii_digit() || ('a'..='f').contains(&value)));
    }

    #[test]
    fn players_find_matches_case_insensitively_and_misses_without_panicking() {
        let mut registry = bridge_player_registry();
        players_register_in_store(&mut registry, "Ada Lovelace").expect("ada");
        let hit = players_find_record(&registry, "  ada lovelace ").expect("hit");
        assert_eq!(hit.id, 1);
        assert_eq!(hit.name, "Ada Lovelace");
        assert!(players_find_record(&registry, "Grace").is_none());
        assert!(players_find_record(&registry, "").is_none());
        assert!(players_find_record(&registry, "a\u{0}b").is_none());
    }

    #[test]
    fn players_reset_when_the_bridge_seed_changes() {
        let mut registry = bridge_player_registry();
        players_register_in_store(&mut registry, "Ada").expect("ada");
        assert!(!player_registry_sync_seed(
            &mut registry,
            BRIDGE_PLAYER_DEFAULT_SEED
        ));
        assert_eq!(registry.count(), 1, "the same seed keeps the names");
        assert!(player_registry_sync_seed(&mut registry, 7));
        assert_eq!(registry.seed, 7);
        assert!(registry.is_empty());
        let player = players_register_in_store(&mut registry, "Ada").expect("ada on the new seed");
        assert_eq!(player.id.get(), 1, "ids restart with a new seed");
    }

    #[test]
    fn bridge_player_seed_requires_open_layers_to_agree() {
        assert_eq!(
            bridge_player_seed([None, None]),
            Ok(BRIDGE_PLAYER_DEFAULT_SEED)
        );
        assert_eq!(bridge_player_seed([None, Some(7), None]), Ok(7));
        assert_eq!(bridge_player_seed([Some(7), None, Some(7)]), Ok(7));
        let (code, _) = bridge_player_seed([Some(7), Some(8)]).expect_err("mismatch");
        assert_eq!(code, "player.seed_mismatch");
    }

    #[test]
    fn bridge_player_names_are_labels_not_paths() {
        let mut registry = bridge_player_registry();
        let label = "../../etc/passwd";
        let player = players_register_in_store(&mut registry, label).expect("label");
        assert_eq!(player.display_name, label);
        let snapshot = players_snapshot(&registry);
        assert_eq!(snapshot.count, 1);
        assert_eq!(snapshot.players[0].name, label);
    }
}
