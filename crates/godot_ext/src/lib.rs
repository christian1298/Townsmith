//! Minimal Godot 4 bridge for Phase 0.
//!
//! The bridge deliberately exposes a coarse API: it builds meshes in the
//! engine-agnostic Rust core and hands them to Godot as `ArrayMesh` resources.
//! Terrain generation and meshing run on Rust worker threads; all Godot
//! resource creation stays on the main thread.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use godot::classes::mesh::PrimitiveType;
use godot::classes::{ArrayMesh, SurfaceTool};
use godot::prelude::*;

use core_voxel::demo;
use core_voxel::mesh::greedy_mesh;
use core_world::{
    terrain_mesh_lod, TerrainMesh, WorldChunkCoord, WorldGenerator, WORLD_CHUNK_SIZE,
};

const MAX_TERRAIN_WORKERS: usize = 4;

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
}

/// Fixed-size Rust worker pool for terrain generation and meshing.
///
/// Workers only touch Rust data. The main thread converts finished
/// [`TerrainMesh`] values into Godot resources in [`TownsmithCore::poll_terrain_chunk`].
struct TerrainBridge {
    jobs: Sender<TerrainJob>,
    results: Receiver<TerrainResult>,
    next_id: u64,
}

impl TerrainBridge {
    fn new() -> Self {
        let (job_tx, job_rx) = mpsc::channel::<TerrainJob>();
        let (result_tx, result_rx) = mpsc::channel::<TerrainResult>();
        let job_rx = Arc::new(Mutex::new(job_rx));

        let available = thread::available_parallelism().map_or(1, std::num::NonZero::get);
        let worker_count = available.clamp(1, MAX_TERRAIN_WORKERS);
        for _ in 0..worker_count {
            let jobs = Arc::clone(&job_rx);
            let results = result_tx.clone();
            thread::spawn(move || loop {
                let job = {
                    let receiver = jobs.lock().unwrap_or_else(|error| error.into_inner());
                    receiver.recv()
                };
                let Ok(job) = job else {
                    break;
                };

                let generator = WorldGenerator::new(job.seed);
                let mesh = generator
                    .generate(WorldChunkCoord::new(job.chunk_x, job.chunk_z))
                    .ok()
                    .map(|chunk| terrain_mesh_lod(&chunk, job.step));

                if results
                    .send(TerrainResult {
                        id: job.id,
                        chunk_x: job.chunk_x,
                        chunk_z: job.chunk_z,
                        step: job.step,
                        mesh,
                    })
                    .is_err()
                {
                    break;
                }
            });
        }

        Self {
            jobs: job_tx,
            results: result_rx,
            next_id: 1,
        }
    }

    fn request(&mut self, seed: u64, chunk_x: i64, chunk_z: i64, step: usize) -> i64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let job = TerrainJob {
            id,
            seed,
            chunk_x,
            chunk_z,
            step,
        };
        if self.jobs.send(job).is_err() {
            -1
        } else {
            i64::try_from(id).unwrap_or(-1)
        }
    }

    fn poll(&self) -> Option<TerrainResult> {
        self.results.try_recv().ok()
    }
}

/// Coarse bridge object used by the Godot shell.
#[derive(GodotClass)]
#[class(init, base=RefCounted)]
struct TownsmithCore {
    terrain: Mutex<Option<TerrainBridge>>,
}

impl Default for TownsmithCore {
    fn default() -> Self {
        Self {
            terrain: Mutex::new(None),
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
        let buffers = terrain_mesh_lod(&chunk, step);
        terrain_mesh_to_godot(&buffers)
    }

    /// Queue a terrain chunk for worker-thread generation.
    ///
    /// Returns a request id. Results are collected with
    /// [`Self::poll_terrain_chunk`] on the main thread. A result dictionary
    /// contains `id`, `chunk_x`, `chunk_z`, `step`, and `mesh`.
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
    /// Returns an empty `Dictionary` when no result is ready. The caller should
    /// poll in a loop until it receives an empty dictionary.
    #[func]
    fn poll_terrain_chunk(&self) -> VarDictionary {
        let guard = self
            .terrain
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(bridge) = guard.as_ref() else {
            return VarDictionary::new();
        };
        let Some(result) = bridge.poll() else {
            return VarDictionary::new();
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
        output
    }
}

fn clamp_step(step: i64) -> usize {
    usize::try_from(step).map_or(1, |step| step.clamp(1, WORLD_CHUNK_SIZE))
}

fn terrain_mesh_to_godot(buffers: &TerrainMesh) -> Gd<ArrayMesh> {
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
            surface.set_color(Color::from_rgb(
                vertex.color[0],
                vertex.color[1],
                vertex.color[2],
            ));
            surface.add_vertex(Vector3::new(
                vertex.position[0],
                vertex.position[1],
                vertex.position[2],
            ));
        }
    }

    surface.commit().unwrap_or_else(ArrayMesh::new_gd)
}
