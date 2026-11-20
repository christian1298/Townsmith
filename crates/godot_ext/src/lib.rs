//! Minimal Godot 4 bridge for Phase 0.
//!
//! The bridge deliberately exposes a coarse API: it builds meshes in the
//! engine-agnostic Rust core and hands them to Godot as `ArrayMesh` resources.
//! Terrain generation and meshing run on Rust worker threads; all Godot
//! resource creation stays on the main thread.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
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
    load_tsbp, save_tsbp_with_hash, Blueprint, BlueprintMetadata, BlueprintStats, RoomRecord,
};
use core_city::{CityLayout, CityTier};
use core_rooms::{detect_rooms, RoomBounds, RoomMaterials, RoomStats, FLOOR_BAND_M};
use core_voxel::coords::{MaterialId, CHUNK_SIZE, VOXEL_SIZE_M};
use core_voxel::demo;
use core_voxel::mesh::{greedy_mesh, MeshBuffers};
use core_voxel::volume::{VolumeConfig, VolumePos, VoxelVolume, MAX_CHUNKS, MAX_CHUNKS_PER_AXIS};
use core_world::{
    terrain_mesh_lod_with_sampler_and_skirt, TerrainMesh, TerrainPos, WorldChunkCoord,
    WorldGenerator, DEFAULT_SKIRT_DEPTH_M, WORLD_CHUNK_SIZE,
};

const MAX_TERRAIN_WORKERS: usize = 4;

/// Material ids used by the Phase 1 editor palette and room detector.
const EDITOR_WALL: u16 = 1;
const EDITOR_FLOOR: u16 = 2;
const EDITOR_CEILING: u16 = 3;
const EDITOR_WINDOW: u16 = 4;
const EDITOR_DOOR: u16 = 5;

fn editor_room_materials() -> RoomMaterials {
    RoomMaterials::new(
        EDITOR_WALL,
        EDITOR_FLOOR,
        EDITOR_CEILING,
        EDITOR_DOOR,
        EDITOR_WINDOW,
    )
}

/// Build blueprint metadata from the editor volume, including detected rooms.
fn blueprint_from_editor_volume(id: String, name: String, volume: VoxelVolume) -> Blueprint {
    let stats = detect_rooms(&volume, &editor_room_materials(), &[]);
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
    };
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

    fn request(&mut self, seed: u64, chunk_x: i64, chunk_z: i64, step: usize) -> i64 {
        let id = self.next_id;
        self.next_id = if id == i64::MAX { 1 } else { id + 1 };

        let job = TerrainJob {
            id: id as u64,
            seed,
            chunk_x,
            chunk_z,
            step,
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

        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let generator = WorldGenerator::new(job.seed);
            generator
                .generate(WorldChunkCoord::new(job.chunk_x, job.chunk_z))
                .map(|chunk| {
                    terrain_mesh_lod_with_sampler_and_skirt(
                        &chunk,
                        job.step,
                        |x, z| generator.height_at(TerrainPos::new(x, z)),
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
struct EditorState {
    volume: VoxelVolume,
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
}

impl Default for TownsmithCore {
    fn default() -> Self {
        Self {
            terrain: Mutex::new(None),
            editor: Mutex::new(None),
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
        let generator = WorldGenerator::new(seed as u64);
        let Ok(chunk) = generator.generate(WorldChunkCoord::new(chunk_x, chunk_z)) else {
            return ArrayMesh::new_gd();
        };
        let buffers = terrain_mesh_lod_with_sampler_and_skirt(
            &chunk,
            step,
            |x, z| generator.height_at(TerrainPos::new(x, z)),
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
        bridge.request(seed as u64, chunk_x, chunk_z, step)
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
        result.set("solid_voxels", state.volume.solid_count() as i64);
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
        let mut result =
            room_stats_to_dictionary(&detect_rooms(&state.volume, &editor_room_materials(), &[]));
        result.set("ok", true);
        result
    }

    /// Save the current editor volume as a `.tsbp` package.
    ///
    /// Returns `{ ok, path, hash, solid_voxels, bounds, rooms }` on success and
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

        let volume = {
            let guard = self
                .editor
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let Some(state) = guard.as_ref() else {
                result.set("ok", false);
                result.set("error", "no editor volume is open");
                return result;
            };
            state.volume.clone()
        };

        let blueprint = blueprint_from_editor_volume(id, name, volume);
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
        result.set(
            "bounds",
            &PackedFloat32Array::from(blueprint.metadata.bounds_m.to_vec()).to_variant(),
        );
        result.set("rooms", blueprint.metadata.rooms.len() as i64);
        result
    }

    /// Load a `.tsbp` package into the editor, replacing the current volume.
    ///
    /// Returns `{ ok, path, hash, solid_voxels, bounds, rooms, name, id }` on
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
        let bounds = PackedFloat32Array::from(blueprint.metadata.bounds_m.to_vec());
        let rooms = blueprint.metadata.rooms.len() as i64;
        let name = blueprint.metadata.name.clone();
        let id = blueprint.metadata.id.clone();

        let mut guard = self
            .editor
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *guard = Some(EditorState::new(blueprint.volume));

        result.set("ok", true);
        result.set("path", path.to_string_lossy().to_string());
        result.set("hash", hash);
        result.set("solid_voxels", solid_voxels as i64);
        result.set("bounds", &bounds.to_variant());
        result.set("rooms", rooms);
        result.set("name", name);
        result.set("id", id);
        result
    }
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
        let id = bridge.request(2026, 0, 0, 1);
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
            .map(|index| bridge.request(2026, index, 0, 1))
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
        let id = bridge.request(2026, 0, 0, 2);
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
        let blueprint = blueprint_from_editor_volume(
            "local:test_room".to_string(),
            "Test Room".to_string(),
            volume,
        );
        assert_eq!(blueprint.metadata.rooms.len(), 1);
        let room = &blueprint.metadata.rooms[0];
        assert_eq!(room.id, "room_0");
        assert_eq!(room.room_type, "empty");
        assert_eq!(room.floor, 0);
        assert!((room.area_m2 - 14.44).abs() < 0.001);
        assert!((room.volume_m3 - 43.32).abs() < 0.001);
        assert_eq!(room.beds, 0);
        assert_eq!(room.doors, 1);
        assert_eq!(room.windows, 1);
        assert_eq!(blueprint.metadata.solid_voxels, solid);
        assert_eq!(blueprint.metadata.bounds_m, [6.4, 3.2, 6.4]);
        assert!(blueprint.metadata.props.is_empty());
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
        assert_eq!(blueprint.metadata.stats.resident_capacity, 0);
        assert_eq!(blueprint.metadata.stats.job_slots, 0);
        assert_eq!(blueprint.metadata.stats.class, "lower_middle");

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
    fn drop_joins_workers_without_hanging() {
        let bridge = TerrainBridge::new();
        drop(bridge);
    }
}
