//! Terrain surface meshing for the Godot shell.
//!
//! The mesher emits the top surface plus exposed vertical cliffs. It is not a
//! full LOD system; it exists to prove that generated terrain can be streamed
//! into an engine and rendered.

use crate::biomes::Biome;
use crate::chunk::WorldChunk;
use crate::coords::{WATER_LEVEL_M, WORLD_CHUNK_SIZE};

/// One terrain vertex with a biome color.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainVertex {
    /// Position in meters relative to the chunk origin.
    pub position: [f32; 3],
    /// Face normal.
    pub normal: [f32; 3],
    /// Linear RGB color.
    pub color: [f32; 3],
}

/// Engine-neutral terrain triangle mesh.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TerrainMesh {
    /// Triangle vertices.
    pub vertices: Vec<TerrainVertex>,
    /// Triangle indices, three per triangle.
    pub indices: Vec<u32>,
}

impl TerrainMesh {
    /// Number of quads (two triangles each).
    #[must_use]
    pub fn quad_count(&self) -> usize {
        self.indices.len() / 6
    }

    /// Whether the mesh has no geometry.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// Build a terrain mesh for one chunk.
#[must_use]
pub fn terrain_mesh(chunk: &WorldChunk) -> TerrainMesh {
    let mut mesh = TerrainMesh::default();

    for z in 0..WORLD_CHUNK_SIZE {
        for x in 0..WORLD_CHUNK_SIZE {
            let Some(height) = chunk.height(x, z) else {
                continue;
            };
            let color = cell_color(chunk, x, z, height);
            push_top(&mut mesh, x, z, height, color);

            let neighbors = [
                (
                    (x as isize) - 1,
                    z as isize,
                    [-1.0, 0.0, 0.0],
                    x as f32,
                    z as f32,
                ),
                (
                    (x as isize) + 1,
                    z as isize,
                    [1.0, 0.0, 0.0],
                    (x + 1) as f32,
                    z as f32,
                ),
                (
                    x as isize,
                    (z as isize) - 1,
                    [0.0, 0.0, -1.0],
                    x as f32,
                    z as f32,
                ),
                (
                    x as isize,
                    (z as isize) + 1,
                    [0.0, 0.0, 1.0],
                    x as f32,
                    (z + 1) as f32,
                ),
            ];

            for (nx, nz, normal, wx, wz) in neighbors {
                let neighbor_height = neighbor_height(chunk, nx, nz);
                if neighbor_height < height {
                    push_wall(
                        &mut mesh,
                        wx,
                        wz,
                        neighbor_height,
                        height,
                        normal,
                        dim(color),
                    );
                }
            }
        }
    }

    mesh
}

fn cell_color(chunk: &WorldChunk, x: usize, z: usize, height: u16) -> [f32; 3] {
    if height <= WATER_LEVEL_M {
        return [0.12, 0.30, 0.55];
    }
    match chunk.biome(x, z).unwrap_or(Biome::Plains) {
        Biome::Plains => [0.32, 0.55, 0.24],
        Biome::Forest => [0.18, 0.40, 0.18],
        Biome::Hills => [0.48, 0.47, 0.42],
        Biome::Coast => [0.72, 0.67, 0.45],
        Biome::RiverValley => [0.25, 0.55, 0.38],
    }
}

fn dim(color: [f32; 3]) -> [f32; 3] {
    [color[0] * 0.72, color[1] * 0.72, color[2] * 0.72]
}

fn neighbor_height(chunk: &WorldChunk, x: isize, z: isize) -> u16 {
    if x < 0 || z < 0 || x >= WORLD_CHUNK_SIZE as isize || z >= WORLD_CHUNK_SIZE as isize {
        0
    } else {
        chunk.height(x as usize, z as usize).unwrap_or(0)
    }
}

fn push_top(mesh: &mut TerrainMesh, x: usize, z: usize, height: u16, color: [f32; 3]) {
    let y = f32::from(height);
    let x0 = x as f32;
    let x1 = (x + 1) as f32;
    let z0 = z as f32;
    let z1 = (z + 1) as f32;
    push_quad(
        mesh,
        [[x0, y, z0], [x0, y, z1], [x1, y, z1], [x1, y, z0]],
        [0.0, 1.0, 0.0],
        color,
    );
}

fn push_wall(
    mesh: &mut TerrainMesh,
    wx: f32,
    wz: f32,
    bottom: u16,
    top: u16,
    normal: [f32; 3],
    color: [f32; 3],
) {
    let y0 = f32::from(bottom);
    let y1 = f32::from(top);

    // Corners are ordered around the wall; culling is disabled in the preview
    // material until the winding convention is locked.
    let (p0, p1, p2, p3) = if normal[0] != 0.0 {
        (
            [wx, y0, wz],
            [wx, y0, wz + 1.0],
            [wx, y1, wz + 1.0],
            [wx, y1, wz],
        )
    } else {
        (
            [wx, y0, wz],
            [wx + 1.0, y0, wz],
            [wx + 1.0, y1, wz],
            [wx, y1, wz],
        )
    };
    push_quad(mesh, [p0, p1, p2, p3], normal, color);
}

fn push_quad(mesh: &mut TerrainMesh, corners: [[f32; 3]; 4], normal: [f32; 3], color: [f32; 3]) {
    let base = mesh.vertices.len() as u32;
    for position in corners {
        mesh.vertices.push(TerrainVertex {
            position,
            normal,
            color,
        });
    }
    mesh.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coords::{WorldChunkCoord, WORLD_CHUNK_CELLS};

    fn flat_chunk(height: u16) -> WorldChunk {
        WorldChunk::from_parts(
            WorldChunkCoord::new(0, 0),
            vec![height; WORLD_CHUNK_CELLS],
            vec![Biome::Plains; WORLD_CHUNK_CELLS],
        )
        .expect("valid chunk")
    }

    #[test]
    fn flat_chunk_has_top_and_edge_walls() {
        let mesh = terrain_mesh(&flat_chunk(24));
        assert_eq!(mesh.quad_count(), WORLD_CHUNK_CELLS + 4 * WORLD_CHUNK_SIZE);
        assert_eq!(mesh.vertices.len(), mesh.quad_count() * 4);
        assert_eq!(mesh.indices.len(), mesh.quad_count() * 6);
    }

    #[test]
    fn raised_cell_adds_four_walls() {
        let mut chunk = flat_chunk(24);
        chunk.set_height(10, 10, 30).expect("valid cell");
        let mesh = terrain_mesh(&chunk);
        assert_eq!(
            mesh.quad_count(),
            WORLD_CHUNK_CELLS + 4 * WORLD_CHUNK_SIZE + 4
        );
    }

    #[test]
    fn zero_height_has_no_edge_walls() {
        let mesh = terrain_mesh(&flat_chunk(0));
        assert_eq!(mesh.quad_count(), WORLD_CHUNK_CELLS);
    }

    #[test]
    fn vertices_and_indices_are_in_bounds() {
        let mesh = terrain_mesh(&flat_chunk(24));
        let vertex_count = mesh.vertices.len() as u32;
        for &index in &mesh.indices {
            assert!(index < vertex_count);
        }
        assert!(mesh.vertices.iter().all(|v| v.position[1] >= 0.0));
    }
}
