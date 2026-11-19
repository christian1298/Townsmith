//! Greedy meshing for a single chunk.

use crate::chunk::VoxelChunk;
use crate::coords::{MaterialId, AIR, CHUNK_SIZE, VOXEL_SIZE_M};

/// One vertex emitted by the voxel mesher.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshVertex {
    /// Position in meters, relative to the chunk origin.
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
}

/// Build a greedy mesh for all exposed chunk faces.
///
/// Chunk boundaries are treated as exposed. Adjacent chunks should eventually
/// be meshed together or use a boundary sampler to hide internal seams.
#[must_use]
pub fn greedy_mesh(chunk: &VoxelChunk) -> MeshBuffers {
    let mut mesh = MeshBuffers::default();

    for axis in 0..3 {
        for slice in 0..CHUNK_SIZE {
            for positive in [false, true] {
                let mask = build_mask(chunk, axis, slice, positive);
                for rect in greedy_rects(&mask) {
                    emit_quad(&mut mesh, axis, slice, positive, rect);
                }
            }
        }
    }

    mesh
}

#[derive(Debug, Clone, Copy)]
struct GreedyRect {
    u0: usize,
    v0: usize,
    u1: usize,
    v1: usize,
    material: MaterialId,
}

fn build_mask(chunk: &VoxelChunk, axis: usize, slice: usize, positive: bool) -> Vec<MaterialId> {
    let mut mask = vec![AIR; CHUNK_SIZE * CHUNK_SIZE];

    for v in 0..CHUNK_SIZE {
        for u in 0..CHUNK_SIZE {
            let voxel = local_coord(axis, slice, u, v);
            let material = chunk.get(voxel[0], voxel[1], voxel[2]);
            if material == AIR {
                continue;
            }

            let neighbor_is_air =
                if (!positive && slice == 0) || (positive && slice + 1 == CHUNK_SIZE) {
                    true
                } else {
                    let neighbor_slice = if positive { slice + 1 } else { slice - 1 };
                    let neighbor = local_coord(axis, neighbor_slice, u, v);
                    chunk.get(neighbor[0], neighbor[1], neighbor[2]) == AIR
                };

            if neighbor_is_air {
                mask[v * CHUNK_SIZE + u] = material;
            }
        }
    }

    mask
}

fn greedy_rects(mask: &[MaterialId]) -> Vec<GreedyRect> {
    debug_assert_eq!(mask.len(), CHUNK_SIZE * CHUNK_SIZE);
    let mut used = vec![false; mask.len()];
    let mut rects = Vec::new();

    for v in 0..CHUNK_SIZE {
        for u in 0..CHUNK_SIZE {
            let start = v * CHUNK_SIZE + u;
            if used[start] || mask[start] == AIR {
                continue;
            }
            let material = mask[start];

            let mut u1 = u + 1;
            while u1 < CHUNK_SIZE
                && !used[v * CHUNK_SIZE + u1]
                && mask[v * CHUNK_SIZE + u1] == material
            {
                u1 += 1;
            }

            let mut v1 = v + 1;
            'height: while v1 < CHUNK_SIZE {
                for uu in u..u1 {
                    let index = v1 * CHUNK_SIZE + uu;
                    if used[index] || mask[index] != material {
                        break 'height;
                    }
                }
                v1 += 1;
            }

            for vv in v..v1 {
                for uu in u..u1 {
                    used[vv * CHUNK_SIZE + uu] = true;
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

fn emit_quad(mesh: &mut MeshBuffers, axis: usize, slice: usize, positive: bool, rect: GreedyRect) {
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
        mesh.vertices.push(MeshVertex {
            position: local_position(axis, plane, uf, vf),
            normal,
            uv: [uf, vf],
            material: rect.material,
        });
    }

    mesh.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

fn local_coord(axis: usize, slice: usize, u: usize, v: usize) -> [usize; 3] {
    let mut coord = [0; 3];
    coord[axis] = slice;
    coord[(axis + 1) % 3] = u;
    coord[(axis + 2) % 3] = v;
    coord
}

fn local_position(axis: usize, slice: f32, u: f32, v: f32) -> [f32; 3] {
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
}
