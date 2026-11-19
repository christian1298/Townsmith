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
    terrain_mesh_lod(chunk, 1)
}

/// Build a terrain mesh with a sampling stride.
///
/// `step` is the number of 1 m cells per mesh quad. `1` is full detail; `2`
/// halves the vertex count per axis, and `4` is suitable for distant chunks.
/// Heights are sampled at the top-left of each span, and walls are emitted
/// against the neighboring samples. This is deliberately approximate: LOD
/// chunks are only used beyond the near view distance.
///
/// `step` is clamped to `1..=WORLD_CHUNK_SIZE`. The final span on each axis is
/// shortened so non-divisor steps (for example `3`) still end at the chunk
/// border and never place a vertex outside the 256 m chunk.
#[must_use]
pub fn terrain_mesh_lod(chunk: &WorldChunk, step: usize) -> TerrainMesh {
    let step = step.clamp(1, WORLD_CHUNK_SIZE);
    let mut mesh = TerrainMesh::default();

    for z in (0..WORLD_CHUNK_SIZE).step_by(step) {
        let span_z = step.min(WORLD_CHUNK_SIZE - z);
        for x in (0..WORLD_CHUNK_SIZE).step_by(step) {
            let span_x = step.min(WORLD_CHUNK_SIZE - x);
            let Some(height) = chunk.height(x, z) else {
                continue;
            };
            let color = cell_color(chunk, x, z, height);
            push_top_span(&mut mesh, x, z, height, color, span_x, span_z);

            let neighbors = [
                (
                    x as isize - step as isize,
                    z as isize,
                    [-1.0, 0.0, 0.0],
                    x as f32,
                    z as f32,
                    span_z,
                ),
                (
                    x as isize + step as isize,
                    z as isize,
                    [1.0, 0.0, 0.0],
                    (x + span_x) as f32,
                    z as f32,
                    span_z,
                ),
                (
                    x as isize,
                    z as isize - step as isize,
                    [0.0, 0.0, -1.0],
                    x as f32,
                    z as f32,
                    span_x,
                ),
                (
                    x as isize,
                    z as isize + step as isize,
                    [0.0, 0.0, 1.0],
                    x as f32,
                    (z + span_z) as f32,
                    span_x,
                ),
            ];

            for (nx, nz, normal, wx, wz, span) in neighbors {
                let neighbor_height = neighbor_height(chunk, nx, nz);
                if neighbor_height < height {
                    push_wall_span(
                        &mut mesh,
                        [wx, wz],
                        span as f32,
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

fn push_top_span(
    mesh: &mut TerrainMesh,
    x: usize,
    z: usize,
    height: u16,
    color: [f32; 3],
    span_x: usize,
    span_z: usize,
) {
    let y = f32::from(height);
    let x0 = x as f32;
    let x1 = (x + span_x) as f32;
    let z0 = z as f32;
    let z1 = (z + span_z) as f32;
    push_quad(
        mesh,
        [[x0, y, z0], [x0, y, z1], [x1, y, z1], [x1, y, z0]],
        [0.0, 1.0, 0.0],
        color,
    );
}

fn push_wall_span(
    mesh: &mut TerrainMesh,
    origin: [f32; 2],
    span: f32,
    bottom: u16,
    top: u16,
    normal: [f32; 3],
    color: [f32; 3],
) {
    let y0 = f32::from(bottom);
    let y1 = f32::from(top);
    let wx = origin[0];
    let wz = origin[1];

    // Corners are ordered around the wall; culling is disabled in the preview
    // material until the winding convention is locked.
    let (p0, p1, p2, p3) = if normal[0] != 0.0 {
        (
            [wx, y0, wz],
            [wx, y0, wz + span],
            [wx, y1, wz + span],
            [wx, y1, wz],
        )
    } else {
        (
            [wx, y0, wz],
            [wx + span, y0, wz],
            [wx + span, y1, wz],
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
        assert_eq!(mesh, terrain_mesh_lod(&flat_chunk(24), 1));
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

    #[test]
    fn lod_step_reduces_quads_and_keeps_bounds() {
        let chunk = flat_chunk(24);
        let full = terrain_mesh(&chunk);
        let half = terrain_mesh_lod(&chunk, 2);
        let quarter = terrain_mesh_lod(&chunk, 4);

        assert!(half.quad_count() < full.quad_count());
        assert!(quarter.quad_count() < half.quad_count());
        assert_eq!(half.quad_count(), expected_flat_quads(2));
        assert_eq!(quarter.quad_count(), expected_flat_quads(4));

        for mesh in [&half, &quarter] {
            let vertex_count = mesh.vertices.len() as u32;
            assert!(mesh.indices.iter().all(|&index| index < vertex_count));
            assert!(mesh.vertices.iter().all(|v| v.position[1] >= 0.0));
            assert_footprint(mesh);
        }
    }

    #[test]
    fn lod_step_zero_defaults_to_full_detail() {
        let chunk = flat_chunk(24);
        assert_eq!(
            terrain_mesh_lod(&chunk, 0).quad_count(),
            terrain_mesh(&chunk).quad_count()
        );
    }

    #[test]
    fn lod_non_divisor_step_stays_inside_the_chunk() {
        let chunk = flat_chunk(24);
        let mesh = terrain_mesh_lod(&chunk, 3);

        // 86 samples per axis (0, 3, ..., 255); the last span is 1 m wide.
        assert_eq!(mesh.quad_count(), expected_flat_quads(3));
        assert_footprint(&mesh);
        for vertex in &mesh.vertices {
            assert!(vertex.position[0] <= WORLD_CHUNK_SIZE as f32);
            assert!(vertex.position[2] <= WORLD_CHUNK_SIZE as f32);
        }
    }

    #[test]
    fn lod_step_larger_than_the_chunk_is_clamped() {
        let chunk = flat_chunk(24);
        let mesh = terrain_mesh_lod(&chunk, 300);
        let full_stride = terrain_mesh_lod(&chunk, WORLD_CHUNK_SIZE);

        assert_eq!(mesh, full_stride);
        assert_eq!(mesh.quad_count(), 1 + 4);
        assert_footprint(&mesh);
        for vertex in &mesh.vertices {
            assert!(vertex.position[0] <= WORLD_CHUNK_SIZE as f32);
            assert!(vertex.position[2] <= WORLD_CHUNK_SIZE as f32);
        }
    }

    #[test]
    fn lod_raised_cell_walls_span_two_cells() {
        let mut chunk = flat_chunk(24);
        chunk.set_height(10, 10, 30).expect("valid cell");
        let mesh = terrain_mesh_lod(&chunk, 2);

        assert_eq!(mesh.quad_count(), expected_flat_quads(2) + 4);
        assert_footprint(&mesh);

        let walls: Vec<[TerrainVertex; 4]> = mesh
            .indices
            .as_chunks::<6>()
            .0
            .iter()
            .map(|indices| quad_vertices(&mesh, indices[0] as usize))
            .filter(|quad| {
                quad[0].normal[1] == 0.0
                    && quad
                        .iter()
                        .all(|vertex| vertex.position[1] >= 24.0 && vertex.position[1] <= 30.0)
                    && quad.iter().any(|vertex| vertex.position[1] == 30.0)
            })
            .collect();
        assert_eq!(walls.len(), 4);

        for wall in walls {
            let span_x = axis_span(&wall, 0);
            let span_z = axis_span(&wall, 2);
            if wall[0].normal[0] != 0.0 {
                assert_eq!(span_x, 0.0);
                assert_eq!(span_z, 2.0);
            } else {
                assert_eq!(span_x, 2.0);
                assert_eq!(span_z, 0.0);
            }
        }
    }

    #[test]
    fn lod_vertices_stay_on_the_sample_grid() {
        let step = 4;
        let mesh = terrain_mesh_lod(&flat_chunk(24), step);
        for vertex in &mesh.vertices {
            assert_eq!(vertex.position[0] as usize % step, 0);
            assert_eq!(vertex.position[2] as usize % step, 0);
        }
    }

    fn expected_flat_quads(step: usize) -> usize {
        let samples = WORLD_CHUNK_SIZE.div_ceil(step);
        samples * samples + 4 * samples
    }

    fn quad_vertices(mesh: &TerrainMesh, base: usize) -> [TerrainVertex; 4] {
        [
            mesh.vertices[base],
            mesh.vertices[base + 1],
            mesh.vertices[base + 2],
            mesh.vertices[base + 3],
        ]
    }

    fn axis_span(quad: &[TerrainVertex; 4], axis: usize) -> f32 {
        let min = quad
            .iter()
            .map(|vertex| vertex.position[axis])
            .reduce(f32::min)
            .unwrap_or(0.0);
        let max = quad
            .iter()
            .map(|vertex| vertex.position[axis])
            .reduce(f32::max)
            .unwrap_or(0.0);
        max - min
    }

    fn assert_footprint(mesh: &TerrainMesh) {
        let max_x = mesh
            .vertices
            .iter()
            .map(|vertex| vertex.position[0])
            .reduce(f32::max)
            .unwrap_or(0.0);
        let max_z = mesh
            .vertices
            .iter()
            .map(|vertex| vertex.position[2])
            .reduce(f32::max)
            .unwrap_or(0.0);
        assert_eq!(max_x, WORLD_CHUNK_SIZE as f32);
        assert_eq!(max_z, WORLD_CHUNK_SIZE as f32);
    }
}
