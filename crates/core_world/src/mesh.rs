//! Terrain surface meshing for the Godot shell.
//!
//! The mesher emits the top surface plus exposed vertical cliffs. It is not a
//! full LOD system; it exists to prove that generated terrain can be streamed
//! into an engine and rendered.
//!
//! Chunks at different LOD steps can meet at T-junctions that show as thin
//! cracks. [`terrain_mesh_lod_with_sampler_and_skirt`] appends vertical
//! perimeter skirts that hang below the chunk border to hide those cracks. The
//! default [`terrain_mesh_lod_with_sampler`] path stays skirt-free.

use crate::biomes::Biome;
use crate::chunk::WorldChunk;
use crate::coords::{TerrainPos, WATER_LEVEL_M, WORLD_CHUNK_SIZE};

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

/// Default skirt depth in meters used by the streaming shell.
pub const DEFAULT_SKIRT_DEPTH_M: f32 = 2.0;

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
    terrain_mesh_lod_with_sampler(chunk, step, |_, _| 0)
}

/// Build a terrain mesh and sample out-of-chunk heights from a world sampler.
///
/// This is the seam-aware variant used by the streaming shell. The sampler is
/// called only for neighbor samples outside the chunk; inside the chunk the
/// chunk's own heights are used. Pass the deterministic world generator's
/// `height_at` so adjacent chunks agree at the seam even when the neighbor
/// chunk has not been loaded into memory yet.
#[must_use]
pub fn terrain_mesh_lod_with_sampler<F>(
    chunk: &WorldChunk,
    step: usize,
    outside_height: F,
) -> TerrainMesh
where
    F: Fn(i64, i64) -> u16,
{
    let step = step.clamp(1, WORLD_CHUNK_SIZE);
    let origin = chunk.coord().origin_m();
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
                    x as isize + span_x as isize,
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
                    z as isize + span_z as isize,
                    [0.0, 0.0, 1.0],
                    x as f32,
                    (z + span_z) as f32,
                    span_x,
                ),
            ];

            for (nx, nz, normal, wx, wz, span) in neighbors {
                let neighbor_height = sample_height(chunk, origin, &outside_height, nx, nz);
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

/// Build a terrain mesh with optional perimeter skirts.
///
/// A skirt is a vertical quad hanging from each perimeter span down by
/// `skirt_depth_m`. Skirts hide T-junction cracks where a chunk at one LOD step
/// meets a neighbor at another step. They are appended after the regular mesh,
/// so a zero or negative depth produces exactly the same output as
/// [`terrain_mesh_lod_with_sampler`].
///
/// Perimeter spans follow the top-surface sampling grid, including clipped final
/// spans for non-divisor steps (for example `3`). Skirt quads use the same
/// biome color and [`dim`] shade as cliff walls, with outward-facing normals and
/// winding that matches the normal. A non-finite `skirt_depth_m` also disables
/// skirts.
#[must_use]
pub fn terrain_mesh_lod_with_sampler_and_skirt<F>(
    chunk: &WorldChunk,
    step: usize,
    outside_height: F,
    skirt_depth_m: f32,
) -> TerrainMesh
where
    F: Fn(i64, i64) -> u16,
{
    let step = step.clamp(1, WORLD_CHUNK_SIZE);
    let mut mesh = terrain_mesh_lod_with_sampler(chunk, step, &outside_height);

    if !skirt_depth_m.is_finite() || skirt_depth_m <= 0.0 {
        return mesh;
    }

    push_perimeter_skirts(&mut mesh, chunk, step, skirt_depth_m);
    mesh
}

fn push_perimeter_skirts(mesh: &mut TerrainMesh, chunk: &WorldChunk, step: usize, depth: f32) {
    let last_x = last_sample(step);
    let last_z = last_sample(step);
    let chunk_end = WORLD_CHUNK_SIZE as f32;

    // North (-Z) and south (+Z) edges share the same X spans as the top mesh.
    for x in (0..WORLD_CHUNK_SIZE).step_by(step) {
        let span = step.min(WORLD_CHUNK_SIZE - x) as f32;
        if let Some(height) = chunk.height(x, 0) {
            let color = dim(cell_color(chunk, x, 0, height));
            push_skirt_span(
                mesh,
                [x as f32, 0.0],
                span,
                height,
                depth,
                [0.0, 0.0, -1.0],
                color,
            );
        }
        if let Some(height) = chunk.height(x, last_z) {
            let color = dim(cell_color(chunk, x, last_z, height));
            push_skirt_span(
                mesh,
                [x as f32, chunk_end],
                span,
                height,
                depth,
                [0.0, 0.0, 1.0],
                color,
            );
        }
    }

    // West (-X) and east (+X) edges share the same Z spans as the top mesh.
    for z in (0..WORLD_CHUNK_SIZE).step_by(step) {
        let span = step.min(WORLD_CHUNK_SIZE - z) as f32;
        if let Some(height) = chunk.height(0, z) {
            let color = dim(cell_color(chunk, 0, z, height));
            push_skirt_span(
                mesh,
                [0.0, z as f32],
                span,
                height,
                depth,
                [-1.0, 0.0, 0.0],
                color,
            );
        }
        if let Some(height) = chunk.height(last_x, z) {
            let color = dim(cell_color(chunk, last_x, z, height));
            push_skirt_span(
                mesh,
                [chunk_end, z as f32],
                span,
                height,
                depth,
                [1.0, 0.0, 0.0],
                color,
            );
        }
    }
}

/// The top-left sample of the final clipped span on one axis.
const fn last_sample(step: usize) -> usize {
    (WORLD_CHUNK_SIZE - 1) / step * step
}

fn push_skirt_span(
    mesh: &mut TerrainMesh,
    origin: [f32; 2],
    span: f32,
    top: u16,
    depth: f32,
    normal: [f32; 3],
    color: [f32; 3],
) {
    let y_top = f32::from(top);
    let y_bottom = y_top - depth;
    let [x, z] = origin;

    // Pick the top edge direction so that edge x down points along `normal`.
    let (p0, p1) = if normal[0] < 0.0 {
        ([x, y_top, z + span], [x, y_top, z])
    } else if normal[0] > 0.0 {
        ([x, y_top, z], [x, y_top, z + span])
    } else if normal[2] < 0.0 {
        ([x, y_top, z], [x + span, y_top, z])
    } else {
        ([x + span, y_top, z], [x, y_top, z])
    };

    push_quad(
        mesh,
        [p0, p1, [p1[0], y_bottom, p1[2]], [p0[0], y_bottom, p0[2]]],
        normal,
        color,
    );
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

fn sample_height<F>(
    chunk: &WorldChunk,
    origin: TerrainPos,
    outside_height: &F,
    x: isize,
    z: isize,
) -> u16
where
    F: Fn(i64, i64) -> u16,
{
    if x < 0 || z < 0 || x >= WORLD_CHUNK_SIZE as isize || z >= WORLD_CHUNK_SIZE as isize {
        outside_height(origin.x + x as i64, origin.z + z as i64)
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
        flat_chunk_at(WorldChunkCoord::new(0, 0), height)
    }

    fn flat_chunk_at(coord: WorldChunkCoord, height: u16) -> WorldChunk {
        WorldChunk::from_parts(
            coord,
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

    #[test]
    fn seam_sampler_culls_matching_neighbor_walls() {
        let chunk = flat_chunk(24);
        let mesh = terrain_mesh_lod_with_sampler(&chunk, 1, |_, _| 24);
        assert_eq!(mesh.quad_count(), WORLD_CHUNK_CELLS);
    }

    #[test]
    fn seam_sampler_keeps_lower_neighbor_walls() {
        let chunk = flat_chunk(24);
        let mesh = terrain_mesh_lod_with_sampler(&chunk, 1, |x, _| if x >= 256 { 20 } else { 24 });
        assert_eq!(mesh.quad_count(), WORLD_CHUNK_CELLS + WORLD_CHUNK_SIZE);
    }

    #[test]
    fn seam_sampler_is_used_for_lod_perimeter() {
        let chunk = flat_chunk(24);
        let mesh = terrain_mesh_lod_with_sampler(&chunk, 2, |_, _| 20);
        assert_eq!(mesh.quad_count(), 128 * 128 + 4 * 128);
    }

    #[test]
    fn seam_sampler_uses_clipped_span_at_the_plus_x_edge() {
        let chunk = flat_chunk(24);
        let mesh = terrain_mesh_lod_with_sampler(&chunk, 3, |x, _| if x == 256 { 20 } else { 24 });
        assert_eq!(mesh.quad_count(), 86 * 86 + 86);
    }

    #[test]
    fn seam_sampler_uses_clipped_span_at_the_plus_z_edge() {
        let chunk = flat_chunk(24);
        let mesh = terrain_mesh_lod_with_sampler(&chunk, 3, |_, z| if z == 256 { 20 } else { 24 });
        assert_eq!(mesh.quad_count(), 86 * 86 + 86);
    }

    #[test]
    fn seam_sampler_receives_negative_world_coordinates() {
        let chunk = flat_chunk_at(WorldChunkCoord::new(-1, 0), 24);
        let saw_west_edge = std::cell::Cell::new(false);
        let mesh = terrain_mesh_lod_with_sampler(&chunk, 1, |x, z| {
            if x == -257 && z == 0 {
                saw_west_edge.set(true);
            }
            24
        });
        assert!(saw_west_edge.get());
        assert_eq!(mesh.quad_count(), WORLD_CHUNK_CELLS);
    }

    #[test]
    fn skirt_adds_expected_perimeter_quads_at_lod_steps() {
        let chunk = flat_chunk(24);
        for step in [1, 2, 3, 4] {
            let samples = WORLD_CHUNK_SIZE.div_ceil(step);
            let base = terrain_mesh_lod_with_sampler(&chunk, step, |_, _| 24);
            let skirted = terrain_mesh_lod_with_sampler_and_skirt(&chunk, step, |_, _| 24, 2.0);

            assert_eq!(base.quad_count(), samples * samples);
            assert_eq!(skirted.quad_count(), samples * samples + 4 * samples);
            assert_eq!(skirted.vertices.len(), skirted.quad_count() * 4);
            assert_eq!(skirted.indices.len(), skirted.quad_count() * 6);

            // The skirt pass appends geometry; it never edits the base mesh.
            assert!(skirted.vertices.starts_with(&base.vertices));
            assert!(skirted.indices.starts_with(&base.indices));

            let skirts = non_top_quads(&skirted);
            assert_eq!(skirts.len(), 4 * samples);
            assert!(skirts.iter().all(|quad| quad.iter().all(on_perimeter)));
            assert_footprint(&skirted);
        }
    }

    #[test]
    fn zero_or_invalid_skirt_matches_the_sampler_mesh() {
        let chunk = flat_chunk(24);
        for step in [1, 2, 3, 4] {
            let expected = terrain_mesh_lod_with_sampler(&chunk, step, |_, _| 20);
            for depth in [0.0, -2.0, f32::NAN, f32::INFINITY] {
                let actual =
                    terrain_mesh_lod_with_sampler_and_skirt(&chunk, step, |_, _| 20, depth);
                assert_eq!(actual, expected, "step {step}, skirt depth {depth}");
            }
        }
    }

    #[test]
    fn skirt_vertices_and_indices_are_in_bounds() {
        let chunk = flat_chunk(24);
        let depth = 2.0;
        for step in [1, 2, 3, 4] {
            let mesh = terrain_mesh_lod_with_sampler_and_skirt(&chunk, step, |_, _| 20, depth);
            let vertex_count = mesh.vertices.len() as u32;
            assert!(mesh.indices.iter().all(|&index| index < vertex_count));
            for vertex in &mesh.vertices {
                assert!((0.0..=WORLD_CHUNK_SIZE as f32).contains(&vertex.position[0]));
                assert!((0.0..=WORLD_CHUNK_SIZE as f32).contains(&vertex.position[2]));
                assert!(vertex.position[1] >= -depth);
                assert!(vertex.position[1] <= WORLD_CHUNK_SIZE as f32);
            }
        }
    }

    #[test]
    fn skirt_edge_heights_use_the_boundary_row_and_column() {
        let mut chunk = flat_chunk(24);
        for x in 0..WORLD_CHUNK_SIZE {
            for z in 0..WORLD_CHUNK_SIZE {
                let height = 24 + (x % 3) as u16 + (z % 5) as u16;
                chunk.set_height(x, z, height).expect("valid cell");
            }
        }

        let step = 4;
        let last = last_sample(step);
        // Mirror the nearest boundary sample so the regular perimeter walls are
        // culled; the remaining boundary quads are exactly the skirts.
        let outside = |x: i64, z: i64| {
            let x = if x < 0 {
                0
            } else if x >= WORLD_CHUNK_SIZE as i64 {
                last
            } else {
                x as usize
            };
            let z = if z < 0 {
                0
            } else if z >= WORLD_CHUNK_SIZE as i64 {
                last
            } else {
                z as usize
            };
            chunk.height(x, z).expect("in-chunk sample")
        };

        let mesh = terrain_mesh_lod_with_sampler_and_skirt(&chunk, step, outside, 2.0);
        let skirts: Vec<[TerrainVertex; 4]> = non_top_quads(&mesh)
            .into_iter()
            .filter(|quad| quad.iter().all(on_perimeter))
            .collect();

        assert_eq!(skirts.len(), 4 * WORLD_CHUNK_SIZE.div_ceil(step));
        for quad in skirts {
            let top = quad
                .iter()
                .map(|vertex| vertex.position[1])
                .fold(f32::NEG_INFINITY, f32::max);
            let bottom = quad
                .iter()
                .map(|vertex| vertex.position[1])
                .fold(f32::INFINITY, f32::min);
            assert_eq!(bottom, top - 2.0);

            let min_x = quad
                .iter()
                .map(|vertex| vertex.position[0])
                .fold(f32::INFINITY, f32::min);
            let min_z = quad
                .iter()
                .map(|vertex| vertex.position[2])
                .fold(f32::INFINITY, f32::min);
            let normal = quad[0].normal;

            if normal[0] < 0.0 {
                assert!(quad.iter().all(|vertex| vertex.position[0] == 0.0));
                assert_eq!(top, f32::from(chunk.height(0, min_z as usize).unwrap()));
            } else if normal[0] > 0.0 {
                assert!(quad
                    .iter()
                    .all(|vertex| vertex.position[0] == WORLD_CHUNK_SIZE as f32));
                assert_eq!(top, f32::from(chunk.height(last, min_z as usize).unwrap()));
            } else if normal[2] < 0.0 {
                assert!(quad.iter().all(|vertex| vertex.position[2] == 0.0));
                assert_eq!(top, f32::from(chunk.height(min_x as usize, 0).unwrap()));
            } else {
                assert!(quad
                    .iter()
                    .all(|vertex| vertex.position[2] == WORLD_CHUNK_SIZE as f32));
                assert_eq!(top, f32::from(chunk.height(min_x as usize, last).unwrap()));
            }
        }
    }

    #[test]
    fn interior_samples_do_not_get_skirts() {
        let mut chunk = flat_chunk(24);
        chunk.set_height(10, 10, 40).expect("valid cell");
        let base = terrain_mesh_lod_with_sampler(&chunk, 1, |_, _| 24);
        let skirted = terrain_mesh_lod_with_sampler_and_skirt(&chunk, 1, |_, _| 24, 2.0);

        assert!(skirted.vertices.starts_with(&base.vertices));
        assert!(skirted.indices.starts_with(&base.indices));

        let appended = &skirted.vertices[base.vertices.len()..];
        assert_eq!(appended.len(), 4 * WORLD_CHUNK_SIZE * 4);
        assert!(appended.iter().all(on_perimeter));
        // The interior bump must not leak into the perimeter skirt heights.
        assert!(appended.iter().all(|vertex| vertex.position[1] <= 24.0));
    }

    #[test]
    fn skirt_normals_face_outward_and_winding_matches() {
        let mesh = terrain_mesh_lod_with_sampler_and_skirt(&flat_chunk(24), 2, |_, _| 24, 2.0);
        let skirts = non_top_quads(&mesh);
        assert_eq!(skirts.len(), 4 * 128);

        for quad in skirts {
            let normal = quad[0].normal;
            let outward = if normal[0] < 0.0 {
                quad.iter().all(|vertex| vertex.position[0] == 0.0)
            } else if normal[0] > 0.0 {
                quad.iter()
                    .all(|vertex| vertex.position[0] == WORLD_CHUNK_SIZE as f32)
            } else if normal[2] < 0.0 {
                quad.iter().all(|vertex| vertex.position[2] == 0.0)
            } else {
                quad.iter()
                    .all(|vertex| vertex.position[2] == WORLD_CHUNK_SIZE as f32)
            };
            assert!(outward);
            assert_eq!(normal_length(normal), 1.0);

            let p0 = quad[0].position;
            let p1 = quad[1].position;
            let p2 = quad[2].position;
            let cross = cross(sub(p1, p0), sub(p2, p0));
            assert!(dot(cross, normal) > 0.0);
        }
    }

    fn expected_flat_quads(step: usize) -> usize {
        let samples = WORLD_CHUNK_SIZE.div_ceil(step);
        samples * samples + 4 * samples
    }

    fn non_top_quads(mesh: &TerrainMesh) -> Vec<[TerrainVertex; 4]> {
        mesh.indices
            .as_chunks::<6>()
            .0
            .iter()
            .map(|indices| quad_vertices(mesh, indices[0] as usize))
            .filter(|quad| quad[0].normal[1] == 0.0)
            .collect()
    }

    fn on_perimeter(vertex: &TerrainVertex) -> bool {
        let max = WORLD_CHUNK_SIZE as f32;
        vertex.position[0] == 0.0
            || vertex.position[0] == max
            || vertex.position[2] == 0.0
            || vertex.position[2] == max
    }

    fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
    }

    fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    }

    fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    fn normal_length(normal: [f32; 3]) -> f32 {
        dot(normal, normal).sqrt()
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
