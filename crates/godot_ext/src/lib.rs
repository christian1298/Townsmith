//! Minimal Godot 4 bridge for Phase 0.
//!
//! The bridge deliberately exposes a coarse API: it builds a demo mesh from the
//! engine-agnostic voxel core and hands it to Godot as an `ArrayMesh`. The core
//! never depends on Godot.

use godot::classes::mesh::PrimitiveType;
use godot::classes::{ArrayMesh, SurfaceTool};
use godot::prelude::*;

use core_voxel::demo;
use core_voxel::mesh::greedy_mesh;
use core_world::{terrain_mesh_lod, TerrainMesh, WorldChunkCoord, WorldGenerator};

struct TownsmithExtension;

#[gdextension]
unsafe impl ExtensionLibrary for TownsmithExtension {}

/// Coarse bridge object used by the Godot shell.
#[derive(GodotClass)]
#[class(init, base=RefCounted)]
struct TownsmithCore;

#[godot_api]
impl TownsmithCore {
    /// Core version string, useful in logs.
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

    /// Generate one 256 m terrain chunk and return it as a colored Godot mesh.
    #[func]
    fn build_terrain_mesh(&self, seed: i64, chunk_x: i64, chunk_z: i64) -> Gd<ArrayMesh> {
        self.build_terrain_mesh_lod(seed, chunk_x, chunk_z, 1)
    }

    /// Generate one terrain chunk at a mesh LOD stride.
    ///
    /// `step` is in 1 m terrain cells: 1 = full detail, 2 = half resolution,
    /// 4 = quarter resolution. Distant chunks use larger steps.
    #[func]
    fn build_terrain_mesh_lod(
        &self,
        seed: i64,
        chunk_x: i64,
        chunk_z: i64,
        step: i64,
    ) -> Gd<ArrayMesh> {
        let generator = WorldGenerator::new(seed as u64);
        let Ok(chunk) = generator.generate(WorldChunkCoord::new(chunk_x, chunk_z)) else {
            return ArrayMesh::new_gd();
        };
        let buffers = terrain_mesh_lod(&chunk, usize::try_from(step).unwrap_or(1));
        terrain_mesh_to_godot(&buffers)
    }
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
