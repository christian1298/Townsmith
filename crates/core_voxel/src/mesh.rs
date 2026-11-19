//! Greedy meshing for a single chunk or a whole voxel volume.

use crate::chunk::VoxelChunk;
use crate::coords::{MaterialId, AIR, CHUNK_SIZE, VOXEL_SIZE_M};

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// One vertex emitted by the voxel mesher.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshVertex {
    /// Position in meters, relative to the mesh origin.
    ///
    /// [`greedy_mesh`] uses the chunk origin; [`crate::volume::VoxelVolume::mesh`]
    /// uses the volume origin.
    pub position: [f32; 3],
    /// Unit face normal.
    pub normal: [f32; 3],
    /// Texture coordinates in meters so materials tile at a fixed scale.
    pub uv: [f32; 2],
    /// Material identifier used by the shell to choose a texture layer.
    pub material: MaterialId,
}

/// Engine-neutral triangle mesh.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshBuffers {
    /// Triangle vertices.
    pub vertices: Vec<MeshVertex>,
    /// Triangle indices, three per triangle.
    pub indices: Vec<u32>,
}

impl MeshBuffers {
    /// Number of quads (two triangles each) represented by the index buffer.
    #[must_use]
    pub fn quad_count(&self) -> usize {
        self.indices.len() / 6
    }

    /// Whether the mesh has no geometry.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Deterministic FNV-1a checksum over every vertex and index.
    ///
    /// The checksum depends only on mesh data, so it can be used for golden
    /// tests and benchmark comparisons across runs and machines.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let mut hash = FNV_OFFSET_BASIS;
        hash = hash_u64(hash, self.vertices.len() as u64);
        for vertex in &self.vertices {
            for axis in vertex.position {
                hash = hash_u64(hash, u64::from(axis.to_bits()));
            }
            for axis in vertex.normal {
                hash = hash_u64(hash, u64::from(axis.to_bits()));
            }
            for axis in vertex.uv {
                hash = hash_u64(hash, u64::from(axis.to_bits()));
            }
            hash = hash_u64(hash, u64::from(vertex.material));
        }

        hash = hash_u64(hash, self.indices.len() as u64);
        for &index in &self.indices {
            hash = hash_u64(hash, u64::from(index));
        }
        hash
    }
}

/// Build a greedy mesh for all exposed chunk faces.
///
/// Chunk boundaries are treated as exposed. Use
/// [`greedy_mesh_with_sampler`] when adjacent chunks are available and faces at
/// the boundary should be culled.
#[must_use]
pub fn greedy_mesh(chunk: &VoxelChunk) -> MeshBuffers {
    greedy_mesh_with_sampler(chunk, |_, _, _| AIR)
}

/// Build a greedy mesh for a chunk while sampling neighbors outside the chunk.
///
/// `sampler` receives chunk-local voxel coordinates. Coordinates may be `-1` or
/// [`CHUNK_SIZE`] on exactly one axis when a face lies on the chunk boundary;
/// a value of `32` means the first voxel of the positive neighbor and `-1` the
/// last voxel of the negative neighbor. To resolve an out-of-range axis,
/// first shift to the adjacent chunk, then map the coordinate with
/// `(coord as usize).rem_euclid(CHUNK_SIZE)` (`CHUNK_SIZE` is `usize`). Return
/// [`AIR`] for unloaded or empty neighbors. Faces whose outside neighbor is
/// solid are culled, but coplanar quads are only merged within this chunk;
/// [`crate::volume::VoxelVolume::mesh`] meshes the whole volume at once so
/// greedy rectangles can also span chunk boundaries.
#[must_use]
pub fn greedy_mesh_with_sampler<F>(chunk: &VoxelChunk, sampler: F) -> MeshBuffers
where
    F: Fn(i32, i32, i32) -> MaterialId,
{
    greedy_mesh_grid([CHUNK_SIZE; 3], [0.0; 3], |coord| {
        if coord.iter().all(|&value| in_chunk(value)) {
            chunk.get(coord[0] as usize, coord[1] as usize, coord[2] as usize)
        } else {
            sampler(coord[0], coord[1], coord[2])
        }
    })
}

/// Mesh an axis-aligned grid, merging coplanar faces of the same material.
///
/// `sample` receives coordinates that may be outside the grid (`-1` or one past
/// the last slice on a single axis); returning [`AIR`] there treats the grid
/// boundary as exposed. Vertices are offset by `origin_m` and reported in
/// meters.
pub(crate) fn greedy_mesh_grid<F>(dims: [usize; 3], origin_m: [f32; 3], sample: F) -> MeshBuffers
where
    F: Fn([i32; 3]) -> MaterialId,
{
    let mut mesh = MeshBuffers::default();
    if dims.contains(&0) {
        return mesh;
    }

    for axis in 0..3 {
        let width = dims[(axis + 1) % 3];
        let height = dims[(axis + 2) % 3];
        for slice in 0..dims[axis] {
            for positive in [false, true] {
                let mask = build_mask(dims, axis, slice, positive, &sample);
                for rect in greedy_rects(&mask, width, height) {
                    emit_quad(&mut mesh, axis, slice, positive, rect, origin_m);
                }
            }
        }
    }

    mesh
}

fn in_chunk(value: i32) -> bool {
    (0..CHUNK_SIZE as i32).contains(&value)
}

fn hash_u64(mut hash: u64, value: u64) -> u64 {
    for byte in value.to_le_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[derive(Debug, Clone, Copy)]
struct GreedyRect {
    u0: usize,
    v0: usize,
    u1: usize,
    v1: usize,
    material: MaterialId,
}

fn build_mask<F>(
    dims: [usize; 3],
    axis: usize,
    slice: usize,
    positive: bool,
    sample: &F,
) -> Vec<MaterialId>
where
    F: Fn([i32; 3]) -> MaterialId,
{
    let width = dims[(axis + 1) % 3];
    let height = dims[(axis + 2) % 3];
    let mut mask = vec![AIR; width * height];

    for v in 0..height {
        for u in 0..width {
            let voxel = grid_coord(axis, slice as i32, u as i32, v as i32);
            let material = sample(voxel);
            if material == AIR {
                continue;
            }

            let neighbor_slice = if positive {
                slice as i32 + 1
            } else {
                slice as i32 - 1
            };
            let neighbor = grid_coord(axis, neighbor_slice, u as i32, v as i32);
            if sample(neighbor) == AIR {
                mask[v * width + u] = material;
            }
        }
    }

    mask
}

fn greedy_rects(mask: &[MaterialId], width: usize, height: usize) -> Vec<GreedyRect> {
    debug_assert_eq!(mask.len(), width * height);
    let mut used = vec![false; mask.len()];
    let mut rects = Vec::new();

    for v in 0..height {
        for u in 0..width {
            let start = v * width + u;
            if used[start] || mask[start] == AIR {
                continue;
            }
            let material = mask[start];

            let mut u1 = u + 1;
            while u1 < width && !used[v * width + u1] && mask[v * width + u1] == material {
                u1 += 1;
            }

            let mut v1 = v + 1;
            'height: while v1 < height {
                for uu in u..u1 {
                    let index = v1 * width + uu;
                    if used[index] || mask[index] != material {
                        break 'height;
                    }
                }
                v1 += 1;
            }

            for vv in v..v1 {
                for uu in u..u1 {
                    used[vv * width + uu] = true;
                }
            }

            rects.push(GreedyRect {
                u0: u,
                v0: v,
                u1,
                v1,
                material,
            });
        }
    }

    rects
}

fn emit_quad(
    mesh: &mut MeshBuffers,
    axis: usize,
    slice: usize,
    positive: bool,
    rect: GreedyRect,
    origin_m: [f32; 3],
) {
    let base = mesh.vertices.len() as u32;
    let plane = if positive {
        (slice + 1) as f32 * VOXEL_SIZE_M
    } else {
        slice as f32 * VOXEL_SIZE_M
    };

    let mut normal = [0.0; 3];
    normal[axis] = if positive { 1.0 } else { -1.0 };

    let corners: [(usize, usize); 4] = if positive {
        [
            (rect.u0, rect.v0),
            (rect.u1, rect.v0),
            (rect.u1, rect.v1),
            (rect.u0, rect.v1),
        ]
    } else {
        [
            (rect.u0, rect.v0),
            (rect.u0, rect.v1),
            (rect.u1, rect.v1),
            (rect.u1, rect.v0),
        ]
    };

    for (u, v) in corners {
        let uf = u as f32 * VOXEL_SIZE_M;
        let vf = v as f32 * VOXEL_SIZE_M;
        let local = grid_position(axis, plane, uf, vf);
        mesh.vertices.push(MeshVertex {
            position: [
                local[0] + origin_m[0],
                local[1] + origin_m[1],
                local[2] + origin_m[2],
            ],
            normal,
            uv: [uf, vf],
            material: rect.material,
        });
    }

    mesh.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

fn grid_coord(axis: usize, slice: i32, u: i32, v: i32) -> [i32; 3] {
    let mut coord = [0; 3];
    coord[axis] = slice;
    coord[(axis + 1) % 3] = u;
    coord[(axis + 2) % 3] = v;
    coord
}

fn grid_position(axis: usize, slice: f32, u: f32, v: f32) -> [f32; 3] {
    let mut position = [0.0; 3];
    position[axis] = slice;
    position[(axis + 1) % 3] = u;
    position[(axis + 2) % 3] = v;
    position
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coords::CHUNK_VOLUME;

    fn assert_indices_in_bounds(mesh: &MeshBuffers) {
        let vertex_count = mesh.vertices.len() as u32;
        for &index in &mesh.indices {
            assert!(index < vertex_count, "index {index} >= {vertex_count}");
        }
    }

    #[test]
    fn empty_chunk_has_no_mesh() {
        let mesh = greedy_mesh(&VoxelChunk::new());
        assert!(mesh.is_empty());
        assert_eq!(mesh.quad_count(), 0);
    }

    #[test]
    fn single_voxel_has_six_quads() {
        let mut chunk = VoxelChunk::new();
        chunk.set(0, 0, 0, 1).unwrap();
        let mesh = greedy_mesh(&chunk);
        assert_eq!(mesh.quad_count(), 6);
        assert_eq!(mesh.vertices.len(), 24);
        assert_eq!(mesh.indices.len(), 36);
        assert_indices_in_bounds(&mesh);
    }

    #[test]
    fn two_adjacent_voxels_share_faces_and_merge_coplanar_quads() {
        let mut chunk = VoxelChunk::new();
        chunk.set(0, 0, 0, 1).unwrap();
        chunk.set(1, 0, 0, 1).unwrap();
        let mesh = greedy_mesh(&chunk);
        assert_eq!(mesh.quad_count(), 6);
        assert_indices_in_bounds(&mesh);
    }

    #[test]
    fn different_materials_are_not_merged() {
        let mut chunk = VoxelChunk::new();
        chunk.set(0, 0, 0, 1).unwrap();
        chunk.set(1, 0, 0, 2).unwrap();
        let mesh = greedy_mesh(&chunk);
        assert_eq!(mesh.quad_count(), 10);
        assert_indices_in_bounds(&mesh);
    }

    #[test]
    fn full_chunk_merges_to_six_boundary_quads() {
        let mut chunk = VoxelChunk::new();
        chunk
            .fill_box([0, 0, 0], [CHUNK_SIZE, CHUNK_SIZE, CHUNK_SIZE], 1)
            .unwrap();
        let mesh = greedy_mesh(&chunk);
        assert_eq!(mesh.quad_count(), 6);
        assert_eq!(mesh.vertices.len(), 24);
        assert_eq!(mesh.indices.len(), 36);
        assert_indices_in_bounds(&mesh);
        assert_eq!(chunk.solid_count() as usize, CHUNK_VOLUME);
    }

    #[test]
    fn normals_point_outward() {
        let mut chunk = VoxelChunk::new();
        chunk.set(5, 5, 5, 1).unwrap();
        let mesh = greedy_mesh(&chunk);
        let mut positive = [0usize; 3];
        let mut negative = [0usize; 3];
        for vertex in &mesh.vertices {
            for axis in 0..3 {
                if vertex.normal[axis] > 0.0 {
                    positive[axis] += 1;
                } else if vertex.normal[axis] < 0.0 {
                    negative[axis] += 1;
                }
            }
        }
        assert_eq!(positive, [4, 4, 4]);
        assert_eq!(negative, [4, 4, 4]);
    }

    #[test]
    fn sampler_treats_missing_neighbor_as_air() {
        let mut chunk = VoxelChunk::new();
        chunk.set(31, 0, 0, 1).unwrap();
        let mesh = greedy_mesh_with_sampler(&chunk, |_, _, _| AIR);
        assert_eq!(mesh.quad_count(), 6);
        assert_indices_in_bounds(&mesh);
    }

    #[test]
    fn sampler_culls_the_face_towards_a_solid_neighbor() {
        let mut left = VoxelChunk::new();
        left.set(31, 0, 0, 1).unwrap();

        // The voxel one step past +X is the first voxel of the right chunk.
        let mesh = greedy_mesh_with_sampler(&left, |x, y, z| {
            if x == CHUNK_SIZE as i32 && y == 0 && z == 0 {
                1
            } else {
                AIR
            }
        });

        assert_eq!(mesh.quad_count(), 5);
        assert_indices_in_bounds(&mesh);
    }

    #[test]
    fn sampler_culls_negative_axis_faces() {
        for (dx, dy, dz) in [(-1, 0, 0), (0, -1, 0), (0, 0, -1)] {
            let mut chunk = VoxelChunk::new();
            chunk.set(0, 0, 0, 1).unwrap();
            let mesh =
                greedy_mesh_with_sampler(
                    &chunk,
                    |x, y, z| {
                        if (x, y, z) == (dx, dy, dz) {
                            1
                        } else {
                            AIR
                        }
                    },
                );
            assert_eq!(mesh.quad_count(), 5, "neighbor {dx},{dy},{dz}");
            assert_indices_in_bounds(&mesh);
        }
    }

    #[test]
    fn checksum_is_stable_and_material_sensitive() {
        let mut chunk = VoxelChunk::new();
        chunk.set(1, 2, 3, 1).unwrap();
        let first = greedy_mesh(&chunk);
        let second = greedy_mesh(&chunk);
        assert_eq!(first, second);
        assert_eq!(first.checksum(), second.checksum());

        chunk.set(1, 2, 3, 2).unwrap();
        let other = greedy_mesh(&chunk);
        assert_ne!(first.checksum(), other.checksum());
    }
}
