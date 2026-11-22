//! Derived road graph and road-distance commute support.
//!
//! Roads are saved as straight, axis-aligned segments. This module turns them
//! into a graph at segment endpoints, T-junctions, and crossings so the city
//! can measure travel along the network instead of straight-line distance. It
//! is derived state: nothing here is persisted or rendered.
//!
//! The graph is deliberately small and deterministic. It splits every segment
//! at crossings and endpoints, deduplicates shared edges, and runs Dijkstra
//! with integer metre weights. Nearest-node lookups use a uniform 32 m grid
//! built with the graph, so point queries do not scan every node. The grid is
//! derived state too: it is never persisted, and the linear scan remains the
//! reference for correctness (and the fallback for queries outside its
//! bounds).

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, VecDeque};
use std::ops::Bound::{Excluded, Unbounded};

use crate::placement::MAX_WORLD_COORD_M;
use crate::road::RoadSegment;

/// Maximum road segments accepted by one traffic graph.
pub const MAX_TRAFFIC_ROADS: usize = 10_000;

/// Default average travel speed for commute time estimates, in metres per
/// second (8.0 m/s = 28.8 km/h).
///
/// This is one flat city-traffic speed, not a per-road or congested speed.
/// It converts the measured road distance into a measurement-layer travel
/// time; it does not feed job assignment.
pub const DEFAULT_TRAVEL_SPEED_MPS: f32 = 8.0;

/// Convert a distance in metres to travel time in seconds at a fixed speed.
///
/// This is a pure helper for the measurement layer. It performs no pathfinding
/// and does not model congestion, signals, or acceleration.
///
/// # Returns
///
/// `None` when `distance_m` or `speed_mps` is not finite, when `distance_m` is
/// negative, or when `speed_mps` is zero or negative. A zero distance at a
/// positive speed returns `Some(0.0)`.
#[must_use]
pub fn travel_time_seconds(distance_m: f32, speed_mps: f32) -> Option<f32> {
    if !distance_m.is_finite() || !speed_mps.is_finite() {
        return None;
    }
    if distance_m < 0.0 || speed_mps <= 0.0 {
        return None;
    }
    let seconds = distance_m / speed_mps;
    seconds.is_finite().then_some(seconds.max(0.0))
}

/// Maximum graph nodes accepted by one traffic graph.
pub const MAX_TRAFFIC_NODES: usize = 20_000;

/// Edge length of one nearest-node index cell, in metres.
///
/// Nodes sit on the 1 m road grid; intersections and endpoints are typically
/// 8–32 m apart and one segment may be up to 512 m long. A 32 m cell holds a
/// handful of nodes per occupied bucket, so a lookup usually resolves in one
/// or two rings while construction stays linear in the node count (one bucket
/// per occupied cell, no pairwise work).
const NEAREST_CELL_M: i32 = 32;

/// Ring cap for the indexed nearest-node search.
///
/// The search normally stops as soon as an arithmetic bound proves that
/// unvisited cells cannot hold a closer or equal node. This cap is a safety
/// valve for pathological graphs (for example two nodes kilometres apart):
/// after it, the exact linear scan runs. 64 rings cover 2,048 m around the
/// query cell.
const NEAREST_MAX_RINGS: i32 = 64;

/// Relative slack for the nearest-node termination proof.
///
/// The index and the linear reference share one `f32` distance expression, so
/// equality must be evaluated on identical values. The bound absorbs `f32`
/// coordinate conversion, subtraction, square, and square-root rounding, whose
/// error is `O((|coordinate| + distance) × 2^-24)`; `1e-5` is a conservative
/// multiple and at most costs an extra ring.
const NEAREST_BOUND_SLACK: f64 = 1e-5;

/// Absolute floor for the termination slack, in metres.
const NEAREST_BOUND_FLOOR_M: f64 = 1e-3;

/// FNV-1a 64-bit offset basis used by graph, field, and route checksums.
pub(crate) const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

/// FNV-1a 64-bit prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Golden-ratio odd multiplier used to mix graph edit counts into versions.
const GRAPH_VERSION_GOLDEN: u64 = 0x9e37_79b9_7f4a_7c15;

pub(crate) fn fnv1a_feed(hash: &mut u64, value: u64) {
    *hash ^= value;
    *hash = hash.wrapping_mul(FNV_PRIME);
}

/// SplitMix64 finalizer: cheap avalanche for version tokens.
const fn mix64(mut value: u64) -> u64 {
    value = value.wrapping_add(GRAPH_VERSION_GOLDEN);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

/// One planar graph node on the 1 m road grid.
pub type RoadNode = [i32; 2];

/// Derived road graph statistics.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TrafficStats {
    /// Road segments that contributed an edge.
    pub roads: usize,
    /// Unique graph nodes.
    pub nodes: usize,
    /// Unique undirected edges.
    pub edges: usize,
    /// Connected components with at least one node.
    pub components: usize,
    /// Sum of every undirected edge length in metres.
    pub total_length_m: f32,
}

impl TrafficStats {
    /// Whether every node belongs to one connected component.
    #[must_use]
    pub const fn connected(&self) -> bool {
        self.nodes == 0 || self.components == 1
    }
}

/// Errors returned while building or extending a traffic graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrafficError {
    /// Too many road segments for one graph.
    TooManyRoads(usize),
    /// Too many split nodes for one graph.
    TooManyNodes(usize),
    /// A segment cannot be appended to the graph.
    ///
    /// `reason` is one of `zero_length`, `not_axis_aligned`, or
    /// `coordinate_out_of_range`. [`RoadGraph::build`] stays lenient and skips
    /// such records; [`RoadGraph::add_segment`] rejects them so a partially
    /// inserted segment can never corrupt the derived graph.
    InvalidSegment {
        /// Segment id.
        id: u64,
        /// Short machine-readable reason.
        reason: &'static str,
    },
    /// A segment reuses an existing id with different geometry.
    ///
    /// Re-adding the exact same segment is a no-op instead.
    DuplicateRoadId(u64),
}

impl std::fmt::Display for TrafficError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyRoads(count) => {
                write!(
                    formatter,
                    "traffic graph has {count} roads; limit is {MAX_TRAFFIC_ROADS}"
                )
            }
            Self::TooManyNodes(count) => {
                write!(
                    formatter,
                    "traffic graph has {count} nodes; limit is {MAX_TRAFFIC_NODES}"
                )
            }
            Self::InvalidSegment { id, reason } => {
                write!(formatter, "road {id} cannot be appended: {reason}")
            }
            Self::DuplicateRoadId(id) => {
                write!(formatter, "road id {id} is already in the traffic graph")
            }
        }
    }
}

impl std::error::Error for TrafficError {}

/// Uniform grid over graph nodes used by [`RoadGraph::nearest_node`].
///
/// Cells are keyed by floor-divided world coordinates, so negative positions
/// bucket consistently. Only occupied cells are stored: the index costs one
/// entry per occupied cell, never a dense rectangle over the world. Node lists
/// inherit the lexicographic order of the graph's node list, which keeps scans
/// deterministic.
#[derive(Debug, Clone, PartialEq, Default)]
struct NodeGrid {
    cells: BTreeMap<(i32, i32), Vec<RoadNode>>,
    min_cell: [i32; 2],
    max_cell: [i32; 2],
    max_abs_coord: f64,
}

impl NodeGrid {
    /// Add one node to its cell, keeping bucket order lexicographic.
    ///
    /// This is the incremental twin of [`Self::build`]: the same node set
    /// produces the same cells, bounds, and `max_abs_coord` regardless of
    /// insertion order.
    fn insert(&mut self, node: RoadNode) {
        let cell = [node_cell(node[0]), node_cell(node[1])];
        if self.cells.is_empty() {
            self.min_cell = cell;
            self.max_cell = cell;
        } else {
            self.min_cell[0] = self.min_cell[0].min(cell[0]);
            self.min_cell[1] = self.min_cell[1].min(cell[1]);
            self.max_cell[0] = self.max_cell[0].max(cell[0]);
            self.max_cell[1] = self.max_cell[1].max(cell[1]);
        }
        let cell_nodes = self.cells.entry((cell[0], cell[1])).or_default();
        let index = cell_nodes.partition_point(|&existing| existing < node);
        cell_nodes.insert(index, node);
        self.max_abs_coord = self
            .max_abs_coord
            .max(f64::from(node[0].unsigned_abs()))
            .max(f64::from(node[1].unsigned_abs()));
    }

    fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Whether a query cell is close enough to the node bounds to search.
    ///
    /// One cell of margin lets points just off the network use the index.
    /// Anything farther falls back to the linear scan instead of expanding
    /// rings across empty space.
    fn contains_query_cell(&self, cell: [i32; 2]) -> bool {
        cell[0] >= self.min_cell[0].saturating_sub(1)
            && cell[0] <= self.max_cell[0].saturating_add(1)
            && cell[1] >= self.min_cell[1].saturating_sub(1)
            && cell[1] <= self.max_cell[1].saturating_add(1)
    }
}

fn node_cell(value: i32) -> i32 {
    value.div_euclid(NEAREST_CELL_M)
}

/// Manhattan length of an axis-aligned graph edge.
fn edge_length(a: RoadNode, b: RoadNode) -> u32 {
    u32::try_from(
        (i64::from(b[0]) - i64::from(a[0])).unsigned_abs()
            + (i64::from(b[1]) - i64::from(a[1])).unsigned_abs(),
    )
    .unwrap_or(u32::MAX)
}

fn query_cell(value: f32) -> i32 {
    (f64::from(value) / f64::from(NEAREST_CELL_M)).floor() as i32
}

/// Straight-line distance from `point` to `node`, in the exact `f32`
/// expression used by the linear reference scan.
fn node_distance(node: RoadNode, point: [f32; 2]) -> f32 {
    let dx = node[0] as f32 - point[0];
    let dz = node[1] as f32 - point[1];
    (dx * dx + dz * dz).sqrt()
}

/// Whether `(node, distance)` beats the current best.
///
/// Equal distances prefer the lexicographically smallest node. This is the
/// single tie-break rule shared by the indexed search and the linear
/// reference, so both return the same node and the same `f32` distance.
fn closer(node: RoadNode, distance: f32, best: Option<(RoadNode, f32)>) -> bool {
    best.is_none_or(|(best_node, best_distance)| {
        distance < best_distance || (distance == best_distance && node < best_node)
    })
}

/// Whether every point outside the visited cell rectangle is farther than
/// `best_distance`, allowing for `f32` rounding in the distance expression.
///
/// The query is inside the rectangle because it sits in the centre cell. The
/// nearest point outside is therefore on one of the four rectangle edges, and
/// the smallest edge distance is a lower bound for every unvisited cell.
fn ring_proves_nearest(
    min_cell: [i32; 2],
    max_cell: [i32; 2],
    point: [f32; 2],
    best_distance: f32,
    max_abs_coord: f64,
) -> bool {
    let cell = f64::from(NEAREST_CELL_M);
    let min_x = f64::from(min_cell[0]) * cell;
    let max_x = f64::from(max_cell[0].saturating_add(1)) * cell;
    let min_z = f64::from(min_cell[1]) * cell;
    let max_z = f64::from(max_cell[1].saturating_add(1)) * cell;
    let x = f64::from(point[0]);
    let z = f64::from(point[1]);
    let outside = (x - min_x).min(max_x - x).min(z - min_z).min(max_z - z);
    let distance = f64::from(best_distance);
    let coordinate = max_abs_coord.max(x.abs()).max(z.abs());
    let slack = (coordinate + distance).mul_add(NEAREST_BOUND_SLACK, NEAREST_BOUND_FLOOR_M);
    outside > distance + slack
}

/// A derived graph over the saved road network.
///
/// The graph is normally built once with [`Self::build`]. When roads are only
/// appended, [`Self::add_segment`] updates the adjacency, node list, nearest
/// node index, and road count in place instead of rebuilding. Removing or
/// changing a road still requires a full rebuild.
///
/// Every graph carries a cheap [`Self::change_version`] token that identifies
/// its current contents and edit lineage. Caches keyed by graph content (for
/// example [`FlowFieldCache`]) use it to reject fields built from another
/// graph.
#[derive(Debug, Clone, PartialEq)]
pub struct RoadGraph {
    adjacency: BTreeMap<RoadNode, Vec<(RoadNode, u32)>>,
    nodes: Vec<RoadNode>,
    grid: NodeGrid,
    /// Accepted segments keyed by id, used to detect duplicate appends.
    roads: BTreeMap<u64, RoadSegment>,
    /// Graph nodes keyed by row (`z`) for constant-time split-point scans.
    row_nodes: BTreeMap<i32, BTreeSet<i32>>,
    /// Graph nodes keyed by column (`x`) for constant-time split-point scans.
    column_nodes: BTreeMap<i32, BTreeSet<i32>>,
    road_count: usize,
    /// Content checksum captured when the graph was built.
    base_version: u64,
    /// Structural appends applied since the base build.
    edit_count: u64,
}

impl Default for RoadGraph {
    fn default() -> Self {
        Self {
            adjacency: BTreeMap::new(),
            nodes: Vec::new(),
            grid: NodeGrid::default(),
            roads: BTreeMap::new(),
            row_nodes: BTreeMap::new(),
            column_nodes: BTreeMap::new(),
            road_count: 0,
            // The checksum of an empty graph is the FNV offset basis.
            base_version: FNV_OFFSET_BASIS,
            edit_count: 0,
        }
    }
}

/// One destination's shortest-path field over the road graph.
///
/// The field stores integer path distance from every reachable node to the
/// goal. Callers reuse it for many homes or agents heading to the same
/// workplace instead of running Dijkstra per pair.
#[derive(Debug, Clone, PartialEq)]
pub struct FlowField {
    goal_point: [f32; 2],
    goal_node: RoadNode,
    goal_access_m: f32,
    distances: BTreeMap<RoadNode, u32>,
}

impl FlowField {
    /// Goal point this field was built for.
    #[must_use]
    pub const fn goal(&self) -> [f32; 2] {
        self.goal_point
    }

    /// Goal graph node.
    #[must_use]
    pub const fn goal_node(&self) -> RoadNode {
        self.goal_node
    }

    /// Number of reachable graph nodes.
    #[must_use]
    pub fn reachable_nodes(&self) -> usize {
        self.distances.len()
    }

    /// FNV-1a checksum over the goal and every reachable distance.
    ///
    /// Two fields built for the same goal on the same graph have the same
    /// checksum. The benchmark uses this to prove that cached reuse returns
    /// exactly the field the graph would have built.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let mut hash = FNV_OFFSET_BASIS;
        fnv1a_feed(&mut hash, u64::from(self.goal_point[0].to_bits()));
        fnv1a_feed(&mut hash, u64::from(self.goal_point[1].to_bits()));
        fnv1a_feed(&mut hash, self.goal_node[0] as u32 as u64);
        fnv1a_feed(&mut hash, self.goal_node[1] as u32 as u64);
        fnv1a_feed(&mut hash, u64::from(self.goal_access_m.to_bits()));
        fnv1a_feed(&mut hash, self.distances.len() as u64);
        for (&node, &distance) in &self.distances {
            fnv1a_feed(&mut hash, node[0] as u32 as u64);
            fnv1a_feed(&mut hash, node[1] as u32 as u64);
            fnv1a_feed(&mut hash, u64::from(distance));
        }
        hash
    }
}

/// Bounded reuse cache for flow fields keyed by exact goal point.
///
/// The commute tick builds one field per workplace. When callers revisit a
/// goal, or several destinations snap to the same point, this helper returns
/// the existing field instead of running Dijkstra again. Eviction is FIFO and
/// deterministic: insertion order decides, never hashing or wall time.
///
/// Every entry stores the [`RoadGraph::change_version`] it was built from.
/// [`Self::get_or_build`] returns a field only when that token still matches
/// the graph it is given; on a mismatch it rebuilds the field in place instead
/// of returning stale distances. Calling [`Self::invalidate`] after a road
/// change also works, but is optional: a cache entry from an older graph can
/// never leak out of the safe lookup path.
///
/// [`Self::get`] is a raw lookup that ignores the graph binding. Prefer
/// [`Self::get_for`] or [`Self::get_or_build`] when a road edit may have
/// happened since the field was built.
#[derive(Debug, Clone)]
pub struct FlowFieldCache {
    fields: BTreeMap<[u32; 2], CachedField>,
    order: VecDeque<[u32; 2]>,
    capacity: usize,
    hits: u64,
    misses: u64,
    rebuilds: u64,
}

/// One cached field plus the graph identity token that produced it.
#[derive(Debug, Clone)]
struct CachedField {
    field: FlowField,
    graph_version: u64,
}

impl Default for FlowFieldCache {
    fn default() -> Self {
        Self::new(64)
    }
}

impl FlowFieldCache {
    /// Create a cache holding at most `capacity` fields.
    ///
    /// A capacity of zero is clamped to one so every successful build can be
    /// returned by reference.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            fields: BTreeMap::new(),
            order: VecDeque::new(),
            capacity: capacity.max(1),
            hits: 0,
            misses: 0,
            rebuilds: 0,
        }
    }

    /// Return a cached field for an exact goal point, if present.
    ///
    /// This is a raw lookup: it does not compare the stored graph version with
    /// any graph. Use [`Self::get_for`] or [`Self::get_or_build`] when the road
    /// network may have changed since the field was built.
    #[must_use]
    pub fn get(&self, goal: [f32; 2]) -> Option<&FlowField> {
        self.fields
            .get(&cache_key(goal))
            .map(|cached| &cached.field)
    }

    /// Return a cached field only when it was built from this exact graph
    /// version.
    ///
    /// Does not count a hit or miss and never rebuilds. This is the read-only
    /// companion to [`Self::get_or_build`].
    #[must_use]
    pub fn get_for(&self, graph: &RoadGraph, goal: [f32; 2]) -> Option<&FlowField> {
        let cached = self.fields.get(&cache_key(goal))?;
        (cached.graph_version == graph.change_version()).then_some(&cached.field)
    }

    /// Return the field for `goal`, building or rebuilding it when needed.
    ///
    /// A cached entry is reused only when its stored graph version matches
    /// `graph`. A mismatch counts as a miss and a [`Self::rebuilds`] event,
    /// then the field is rebuilt from `graph` and replaces the stale entry in
    /// place (its FIFO position is unchanged).
    ///
    /// Returns `None` when the goal is not finite or the graph has no node for
    /// it; the miss is still counted. If a stale entry cannot be rebuilt, it is
    /// dropped so no raw [`Self::get`] can return it later. When the cache is
    /// full, the oldest inserted field is evicted first.
    pub fn get_or_build(&mut self, graph: &RoadGraph, goal: [f32; 2]) -> Option<&FlowField> {
        let key = cache_key(goal);
        let graph_version = graph.change_version();
        match self.fields.get(&key).map(|cached| cached.graph_version) {
            Some(cached_version) if cached_version == graph_version => {
                self.hits += 1;
                return self.fields.get(&key).map(|cached| &cached.field);
            }
            Some(_) => {
                self.misses += 1;
                self.rebuilds += 1;
                let Some(field) = graph.flow_to(goal) else {
                    self.fields.remove(&key);
                    self.order.retain(|queued| *queued != key);
                    return None;
                };
                let cached = self
                    .fields
                    .get_mut(&key)
                    .expect("stale cache entry was just read");
                cached.field = field;
                cached.graph_version = graph_version;
                return self.fields.get(&key).map(|cached| &cached.field);
            }
            None => {}
        }
        self.misses += 1;
        let field = graph.flow_to(goal)?;
        if self.fields.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.fields.remove(&oldest);
            }
        }
        self.order.push_back(key);
        self.fields.insert(
            key,
            CachedField {
                field,
                graph_version,
            },
        );
        self.fields.get(&key).map(|cached| &cached.field)
    }

    /// Number of cached fields.
    #[must_use]
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// Whether the cache holds no field.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Maximum number of cached fields.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Lookup hits since creation.
    #[must_use]
    pub const fn hits(&self) -> u64 {
        self.hits
    }

    /// Lookups that built a field, including invalid goals and stale rebuilds.
    #[must_use]
    pub const fn misses(&self) -> u64 {
        self.misses
    }

    /// Fields rebuilt in place because the graph version changed.
    ///
    /// A stale entry counts once here and once in [`Self::misses`]. Replacing
    /// a missing entry and rebuilding after [`Self::invalidate`] do not count.
    #[must_use]
    pub const fn rebuilds(&self) -> u64 {
        self.rebuilds
    }

    /// Drop every cached field. Hit, miss, and rebuild counters are cumulative
    /// for the lifetime of the cache.
    ///
    /// Call after a full graph rebuild for an immediate release; it is not
    /// required for correctness, because entries carry their graph version.
    pub fn invalidate(&mut self) {
        self.fields.clear();
        self.order.clear();
    }

    /// Drop every cached field, keeping cumulative counters.
    ///
    /// Alias for [`Self::invalidate`], retained for existing callers.
    pub fn clear(&mut self) {
        self.invalidate();
    }
}

fn cache_key(goal: [f32; 2]) -> [u32; 2] {
    // Canonicalize ±0.0 so equal coordinates share one entry.
    let bits = |value: f32| {
        if value == 0.0 {
            0.0_f32.to_bits()
        } else {
            value.to_bits()
        }
    };
    [bits(goal[0]), bits(goal[1])]
}

impl RoadGraph {
    /// Build a graph from saved road segments.
    ///
    /// Segments are split at every crossing, T-junction, and shared endpoint.
    /// Zero-length or non-axis-aligned records are ignored; saved roads are
    /// validated before they reach this point.
    pub fn build(roads: &[RoadSegment]) -> Result<Self, TrafficError> {
        if roads.len() > MAX_TRAFFIC_ROADS {
            return Err(TrafficError::TooManyRoads(roads.len()));
        }

        let mut vertical_by_x: BTreeMap<i32, Vec<&RoadSegment>> = BTreeMap::new();
        let mut horizontal_by_z: BTreeMap<i32, Vec<&RoadSegment>> = BTreeMap::new();
        let mut endpoints_by_z: BTreeMap<i32, Vec<i32>> = BTreeMap::new();
        let mut endpoints_by_x: BTreeMap<i32, Vec<i32>> = BTreeMap::new();

        for road in roads {
            let start = road.start;
            let end = road.end;
            let horizontal = start[1] == end[1];
            let vertical = start[0] == end[0];
            if horizontal == vertical {
                continue;
            }
            endpoints_by_z.entry(start[1]).or_default().push(start[0]);
            endpoints_by_z.entry(end[1]).or_default().push(end[0]);
            endpoints_by_x.entry(start[0]).or_default().push(start[1]);
            endpoints_by_x.entry(end[0]).or_default().push(end[1]);
            if horizontal {
                horizontal_by_z.entry(start[1]).or_default().push(road);
            } else {
                vertical_by_x.entry(start[0]).or_default().push(road);
            }
        }

        let mut graph = Self::default();

        for road in roads {
            let start = road.start;
            let end = road.end;
            let horizontal = start[1] == end[1];
            let vertical = start[0] == end[0];
            if horizontal == vertical {
                continue;
            }
            graph.road_count += 1;
            graph.roads.insert(road.id, road.clone());

            let mut points = vec![start, end];
            if horizontal {
                let z = start[1];
                if let Some(xs) = endpoints_by_z.get(&z) {
                    for &x in xs {
                        if x >= start[0].min(end[0]) && x <= start[0].max(end[0]) {
                            points.push([x, z]);
                        }
                    }
                }
                for x in start[0].min(end[0])..=start[0].max(end[0]) {
                    if let Some(verticals) = vertical_by_x.get(&x) {
                        for candidate in verticals {
                            let min_z = candidate.start[1].min(candidate.end[1]);
                            let max_z = candidate.start[1].max(candidate.end[1]);
                            if z >= min_z && z <= max_z {
                                points.push([x, z]);
                            }
                        }
                    }
                }
            } else {
                let x = start[0];
                if let Some(zs) = endpoints_by_x.get(&x) {
                    for &z in zs {
                        if z >= start[1].min(end[1]) && z <= start[1].max(end[1]) {
                            points.push([x, z]);
                        }
                    }
                }
                for z in start[1].min(end[1])..=start[1].max(end[1]) {
                    if let Some(horizontals) = horizontal_by_z.get(&z) {
                        for candidate in horizontals {
                            let min_x = candidate.start[0].min(candidate.end[0]);
                            let max_x = candidate.start[0].max(candidate.end[0]);
                            if x >= min_x && x <= max_x {
                                points.push([x, z]);
                            }
                        }
                    }
                }
            }

            points.sort_unstable_by(|left, right| {
                left[0].cmp(&right[0]).then(left[1].cmp(&right[1]))
            });
            points.dedup();
            graph.connect_points(&points);
        }

        if graph.adjacency.len() > MAX_TRAFFIC_NODES {
            return Err(TrafficError::TooManyNodes(graph.adjacency.len()));
        }
        graph.base_version = graph.checksum();
        Ok(graph)
    }

    /// Append one saved road segment to an already-built graph.
    ///
    /// The split points are recomputed against the current node index: the new
    /// segment is divided at every existing graph node on its line, at every
    /// perpendicular crossing, and at its own endpoints. Existing edges that
    /// contain a new point are split, so the result is identical (checksum,
    /// nearest-node choice, and routes) to a fresh [`Self::build`] over the
    /// previous segments followed by this one.
    ///
    /// Returns `Ok(true)` when at least one new node or edge was inserted. An
    /// exact duplicate id/geometry, or a segment whose geometry is already
    /// fully covered by the graph, is accepted as a no-op and returns
    /// `Ok(false)`; the record still counts toward [`MAX_TRAFFIC_ROADS`], which
    /// mirrors `build`'s segment count.
    ///
    /// Only appends are incremental. Removing a road, changing its geometry,
    /// or re-adding an existing id with different coordinates needs a full
    /// rebuild. An append that inserts no node or edge (a duplicate id and
    /// geometry, or already covered geometry) is a no-op and leaves
    /// [`Self::change_version`] unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`TrafficError::InvalidSegment`] for a zero-length,
    /// non-axis-aligned, or out-of-world segment,
    /// [`TrafficError::DuplicateRoadId`] when the id exists with different
    /// geometry, and the cap errors when the graph would exceed
    /// [`MAX_TRAFFIC_ROADS`] or [`MAX_TRAFFIC_NODES`]. Every error leaves the
    /// graph unchanged.
    pub fn add_segment(&mut self, road: &RoadSegment) -> Result<bool, TrafficError> {
        Self::validate_appended_segment(road)?;
        if let Some(existing) = self.roads.get(&road.id) {
            return if existing == road {
                Ok(false)
            } else {
                Err(TrafficError::DuplicateRoadId(road.id))
            };
        }
        if self.road_count >= MAX_TRAFFIC_ROADS {
            return Err(TrafficError::TooManyRoads(self.road_count + 1));
        }

        let points = self.append_split_points(road);
        let new_nodes = points
            .iter()
            .filter(|&&node| !self.adjacency.contains_key(&node))
            .count();
        let resulting_nodes = self.adjacency.len().saturating_add(new_nodes);
        if resulting_nodes > MAX_TRAFFIC_NODES {
            return Err(TrafficError::TooManyNodes(resulting_nodes));
        }

        for &point in &points {
            self.split_at(point);
        }
        let changed = self.connect_points(&points);
        self.roads.insert(road.id, road.clone());
        self.road_count += 1;
        if changed {
            self.edit_count = self.edit_count.wrapping_add(1);
        }
        Ok(changed)
    }

    /// Append many saved segments, stopping at the first error.
    ///
    /// Returns the number of segments that inserted at least one new node or
    /// edge. Additions before the error remain applied; callers that need
    /// all-or-nothing behavior should rebuild with [`Self::build`], clone the
    /// graph, or validate first. The caps are enforced on every append.
    ///
    /// # Errors
    ///
    /// Returns the first [`TrafficError`] from [`Self::add_segment`].
    pub fn extend_segments(&mut self, roads: &[RoadSegment]) -> Result<usize, TrafficError> {
        let mut added = 0;
        for road in roads {
            if self.add_segment(road)? {
                added += 1;
            }
        }
        Ok(added)
    }

    fn validate_appended_segment(road: &RoadSegment) -> Result<(), TrafficError> {
        let limit = MAX_WORLD_COORD_M as i64;
        if road
            .start
            .iter()
            .chain(road.end.iter())
            .any(|value| i64::from(*value).abs() > limit)
        {
            return Err(TrafficError::InvalidSegment {
                id: road.id,
                reason: "coordinate_out_of_range",
            });
        }
        let dx = i64::from(road.end[0]) - i64::from(road.start[0]);
        let dz = i64::from(road.end[1]) - i64::from(road.start[1]);
        if dx == 0 && dz == 0 {
            return Err(TrafficError::InvalidSegment {
                id: road.id,
                reason: "zero_length",
            });
        }
        if dx != 0 && dz != 0 {
            return Err(TrafficError::InvalidSegment {
                id: road.id,
                reason: "not_axis_aligned",
            });
        }
        Ok(())
    }

    /// Split points for a segment appended to this graph.
    ///
    /// Every existing graph node on the segment's line is a split point, as is
    /// every perpendicular edge crossing it. That matches `build`, where the
    /// global endpoint and crossing tables already contain those points.
    fn append_split_points(&self, road: &RoadSegment) -> Vec<RoadNode> {
        let mut points = BTreeSet::new();
        points.insert(road.start);
        points.insert(road.end);
        if road.start[1] == road.end[1] {
            let z = road.start[1];
            let min_x = road.start[0].min(road.end[0]);
            let max_x = road.start[0].max(road.end[0]);
            if let Some(xs) = self.row_nodes.get(&z) {
                for &x in xs.range(min_x..=max_x) {
                    points.insert([x, z]);
                }
            }
            for (&x, _) in self.column_nodes.range(min_x..=max_x) {
                if self.column_crossing(x, z) {
                    points.insert([x, z]);
                }
            }
        } else {
            let x = road.start[0];
            let min_z = road.start[1].min(road.end[1]);
            let max_z = road.start[1].max(road.end[1]);
            if let Some(zs) = self.column_nodes.get(&x) {
                for &z in zs.range(min_z..=max_z) {
                    points.insert([x, z]);
                }
            }
            for (&z, _) in self.row_nodes.range(min_z..=max_z) {
                if self.row_crossing(x, z) {
                    points.insert([x, z]);
                }
            }
        }
        points.into_iter().collect()
    }

    /// Whether a vertical edge at `x` strictly spans `z`.
    fn column_crossing(&self, x: i32, z: i32) -> bool {
        let Some(zs) = self.column_nodes.get(&x) else {
            return false;
        };
        let Some(&lower) = zs.range(..z).next_back() else {
            return false;
        };
        let Some(&upper) = zs.range((Excluded(z), Unbounded)).next() else {
            return false;
        };
        self.has_edge([x, lower], [x, upper])
    }

    /// Whether a horizontal edge at `z` strictly spans `x`.
    fn row_crossing(&self, x: i32, z: i32) -> bool {
        let Some(xs) = self.row_nodes.get(&z) else {
            return false;
        };
        let Some(&lower) = xs.range(..x).next_back() else {
            return false;
        };
        let Some(&upper) = xs.range((Excluded(x), Unbounded)).next() else {
            return false;
        };
        self.has_edge([lower, z], [upper, z])
    }

    fn has_edge(&self, a: RoadNode, b: RoadNode) -> bool {
        self.adjacency.get(&a).is_some_and(|neighbors| {
            neighbors
                .binary_search_by_key(&b, |&(node, _)| node)
                .is_ok()
        })
    }

    /// Split every existing horizontal or vertical edge that contains `point`.
    fn split_at(&mut self, point: RoadNode) {
        if let Some((lower, upper)) = self.edge_containing(point, true) {
            self.split_edge(lower, upper, point);
        }
        if let Some((lower, upper)) = self.edge_containing(point, false) {
            self.split_edge(lower, upper, point);
        }
    }

    /// The existing edge on `point`'s row or column that strictly contains it.
    fn edge_containing(&self, point: RoadNode, horizontal: bool) -> Option<(RoadNode, RoadNode)> {
        let (line, coordinate) = if horizontal {
            (point[1], point[0])
        } else {
            (point[0], point[1])
        };
        let coordinates = if horizontal {
            self.row_nodes.get(&line)?
        } else {
            self.column_nodes.get(&line)?
        };
        let lower = *coordinates.range(..coordinate).next_back()?;
        let upper = *coordinates
            .range((Excluded(coordinate), Unbounded))
            .next()?;
        let (a, b) = if horizontal {
            ([lower, line], [upper, line])
        } else {
            ([line, lower], [line, upper])
        };
        self.has_edge(a, b).then_some((a, b))
    }

    fn split_edge(&mut self, lower: RoadNode, upper: RoadNode, point: RoadNode) {
        self.remove_edge(lower, upper);
        self.insert_edge(lower, point);
        self.insert_edge(point, upper);
    }

    fn remove_edge(&mut self, a: RoadNode, b: RoadNode) {
        if let Some(neighbors) = self.adjacency.get_mut(&a) {
            if let Ok(index) = neighbors.binary_search_by_key(&b, |&(node, _)| node) {
                neighbors.remove(index);
            }
        }
        if let Some(neighbors) = self.adjacency.get_mut(&b) {
            if let Ok(index) = neighbors.binary_search_by_key(&a, |&(node, _)| node) {
                neighbors.remove(index);
            }
        }
    }

    /// Add an undirected edge, keeping neighbor lists sorted by node.
    ///
    /// Returns whether the edge was new. The length is fixed by the endpoint
    /// geometry, so an existing duplicate keeps its value.
    fn insert_edge(&mut self, a: RoadNode, b: RoadNode) -> bool {
        self.ensure_node(a);
        self.ensure_node(b);
        let length = edge_length(a, b);
        let inserted = {
            let neighbors = self
                .adjacency
                .get_mut(&a)
                .expect("insert_edge ensures both endpoints");
            match neighbors.binary_search_by_key(&b, |&(node, _)| node) {
                Ok(index) => {
                    neighbors[index].1 = neighbors[index].1.min(length);
                    false
                }
                Err(index) => {
                    neighbors.insert(index, (b, length));
                    true
                }
            }
        };
        if inserted {
            let neighbors = self
                .adjacency
                .get_mut(&b)
                .expect("insert_edge ensures both endpoints");
            if let Err(index) = neighbors.binary_search_by_key(&a, |&(node, _)| node) {
                neighbors.insert(index, (a, length));
            }
        }
        inserted
    }

    /// Ensure a node exists in the adjacency, sorted node list, grid, and
    /// row/column indexes.
    fn ensure_node(&mut self, node: RoadNode) -> bool {
        if self.adjacency.contains_key(&node) {
            return false;
        }
        self.adjacency.insert(node, Vec::new());
        let index = self.nodes.partition_point(|&existing| existing < node);
        self.nodes.insert(index, node);
        self.grid.insert(node);
        self.row_nodes.entry(node[1]).or_default().insert(node[0]);
        self.column_nodes
            .entry(node[0])
            .or_default()
            .insert(node[1]);
        true
    }

    /// Insert consecutive edges along one segment's sorted split points.
    ///
    /// Returns whether any node or edge was new. `build` and `add_segment`
    /// share this step, which is what keeps an appended graph identical to a
    /// fresh build.
    fn connect_points(&mut self, points: &[RoadNode]) -> bool {
        let Some(&first) = points.first() else {
            return false;
        };
        let mut changed = self.ensure_node(first);
        for pair in points.windows(2) {
            changed |= self.insert_edge(pair[0], pair[1]);
        }
        changed
    }

    /// Derived graph statistics.
    #[must_use]
    pub fn stats(&self) -> TrafficStats {
        let mut components = 0_usize;
        let mut seen: BTreeSet<RoadNode> = BTreeSet::new();
        for &start in &self.nodes {
            if !seen.insert(start) {
                continue;
            }
            components += 1;
            let mut stack = vec![start];
            while let Some(node) = stack.pop() {
                if let Some(neighbors) = self.adjacency.get(&node) {
                    for &(next, _) in neighbors {
                        if seen.insert(next) {
                            stack.push(next);
                        }
                    }
                }
            }
        }
        let total_length = self
            .adjacency
            .values()
            .flatten()
            .map(|(_, length)| u64::from(*length))
            .sum::<u64>()
            / 2;
        TrafficStats {
            roads: self.road_count,
            nodes: self.nodes.len(),
            edges: self.adjacency.values().map(Vec::len).sum::<usize>() / 2,
            components,
            total_length_m: total_length as f32,
        }
    }

    /// Whether the graph has no usable road.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Distance along the road network between two world points.
    ///
    /// Each point walks to its nearest graph node, then follows the shortest
    /// path. Returns `None` when the points snap to different components.
    /// Coincident points return zero without a walk. Points off the network
    /// still work; the walking legs are included in the result.
    #[must_use]
    pub fn distance_m(&self, from: [f32; 2], to: [f32; 2]) -> Option<f32> {
        if !from.iter().chain(to.iter()).all(|value| value.is_finite()) {
            return None;
        }
        let dx = to[0] - from[0];
        let dz = to[1] - from[1];
        if dx * dx + dz * dz <= 0.000_1 {
            return Some(0.0);
        }
        let (start, start_access) = self.nearest_node(from)?;
        let (goal, goal_access) = self.nearest_node(to)?;
        let path = self.shortest_path(start, goal)?;
        Some(start_access + path as f32 + goal_access)
    }

    /// Polyline from `from` to `to` along the road network.
    ///
    /// Each endpoint walks straight to its nearest graph node; the result is
    /// `from`, the node path in travel order, then `to`. Consecutive duplicate
    /// points are dropped. Coincident endpoints return a two-point degenerate
    /// polyline `[from, to]`, matching [`Self::distance_m`], even when the
    /// graph is empty.
    ///
    /// Distances use the same integer metre weights as [`Self::distance_m`].
    /// Equal-length alternatives resolve to the lexicographically smallest
    /// predecessor node, so repeated builds return the same polyline. Returns
    /// `None` when an endpoint is non-finite, the graph has no nodes, or the
    /// snapped nodes are disconnected.
    #[must_use]
    pub fn route_points(&self, from: [f32; 2], to: [f32; 2]) -> Option<Vec<[f32; 2]>> {
        if !from.iter().chain(to.iter()).all(|value| value.is_finite()) {
            return None;
        }
        let dx = to[0] - from[0];
        let dz = to[1] - from[1];
        if dx * dx + dz * dz <= 0.000_1 {
            return Some(vec![from, to]);
        }
        let (start, _) = self.nearest_node(from)?;
        let (goal, _) = self.nearest_node(to)?;
        let (_, nodes) = self.path_nodes(start, goal)?;

        let mut points = Vec::with_capacity(nodes.len() + 2);
        push_unique_point(&mut points, from);
        for node in nodes {
            push_unique_point(&mut points, [node[0] as f32, node[1] as f32]);
        }
        push_unique_point(&mut points, to);
        Some(points)
    }

    /// Build a flow field from the goal to every reachable node.
    ///
    /// This is the shared-destination primitive: one Dijkstra per workplace,
    /// school, or service instead of one per home/workplace pair.
    #[must_use]
    pub fn flow_to(&self, goal: [f32; 2]) -> Option<FlowField> {
        if !goal.iter().all(|value| value.is_finite()) {
            return None;
        }
        let (goal_node, goal_access_m) = self.nearest_node(goal)?;
        let mut distances: BTreeMap<RoadNode, u32> = BTreeMap::new();
        distances.insert(goal_node, 0);
        let mut heap: BinaryHeap<Reverse<(u32, RoadNode)>> = BinaryHeap::new();
        heap.push(Reverse((0, goal_node)));
        while let Some(Reverse((distance, node))) = heap.pop() {
            if distances.get(&node).is_some_and(|&known| distance > known) {
                continue;
            }
            let Some(neighbors) = self.adjacency.get(&node) else {
                continue;
            };
            for &(next, weight) in neighbors {
                let candidate = distance.saturating_add(weight);
                let known = distances.get(&next).copied();
                if known.is_none_or(|value| candidate < value) {
                    distances.insert(next, candidate);
                    heap.push(Reverse((candidate, next)));
                }
            }
        }
        Some(FlowField {
            goal_point: goal,
            goal_node,
            goal_access_m,
            distances,
        })
    }

    /// Distance from a point to a flow field goal.
    ///
    /// Returns `None` when the point snaps to an unreachable component.
    #[must_use]
    pub fn flow_distance_m(&self, from: [f32; 2], field: &FlowField) -> Option<f32> {
        if !from.iter().all(|value| value.is_finite()) {
            return None;
        }
        let dx = field.goal_point[0] - from[0];
        let dz = field.goal_point[1] - from[1];
        if dx * dx + dz * dz <= 0.000_1 {
            return Some(0.0);
        }
        let (start, start_access) = self.nearest_node(from)?;
        let path = *field.distances.get(&start)?;
        Some(start_access + path as f32 + field.goal_access_m)
    }

    /// Straight-line distance between two points, used as a fallback.
    #[must_use]
    pub fn straight_line_m(from: [f32; 2], to: [f32; 2]) -> f32 {
        let dx = to[0] - from[0];
        let dz = to[1] - from[1];
        (dx * dx + dz * dz).sqrt()
    }

    /// FNV-1a checksum over the derived graph.
    ///
    /// This is the stable content fingerprint used by saves, tests, and the
    /// bridge. It is not the cache identity token: use
    /// [`Self::change_version`] for invalidating derived caches.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let mut hash = FNV_OFFSET_BASIS;
        for &node in &self.nodes {
            fnv1a_feed(&mut hash, node[0] as u32 as u64);
            fnv1a_feed(&mut hash, node[1] as u32 as u64);
            if let Some(neighbors) = self.adjacency.get(&node) {
                for &(next, length) in neighbors {
                    fnv1a_feed(&mut hash, next[0] as u32 as u64);
                    fnv1a_feed(&mut hash, next[1] as u32 as u64);
                    fnv1a_feed(&mut hash, u64::from(length));
                }
            }
        }
        hash
    }

    /// Cheap identity token for the graph's current contents and edit lineage.
    ///
    /// Two graphs built from the same segment list start with the same token.
    /// A structural [`Self::add_segment`] advances the token; a no-op append
    /// leaves it unchanged. The token is derived from the build-time content
    /// checksum and a local edit count, so it is deterministic for a given
    /// construction sequence and needs no I/O or graph scan to read.
    ///
    /// A full rebuild of an appended graph produces a *different* token even
    /// when the content matches (the edit count restarts). That is
    /// conservative: caches keyed on this token rebuild once instead of
    /// risking a stale field, then settle again. Do not use the token as a
    /// save-format checksum; [`Self::checksum`] is the stable content hash.
    #[must_use]
    pub const fn change_version(&self) -> u64 {
        mix64(self.base_version ^ self.edit_count.wrapping_mul(GRAPH_VERSION_GOLDEN))
    }

    /// Nearest graph node to a world point and the straight-line distance to
    /// it, in metres.
    ///
    /// The lookup uses the uniform node grid built with the graph. It expands
    /// cell rings around the query until an arithmetic bound proves that no
    /// unvisited cell can contain a closer node, then returns. Equal distances
    /// resolve to the lexicographically smallest node, exactly matching
    /// [`Self::nearest_node_linear`]. Non-finite points, an empty graph, points
    /// more than one cell outside the node bounds, and pathological sparse
    /// graphs fall back to the exact linear scan.
    #[must_use]
    pub fn nearest_node(&self, point: [f32; 2]) -> Option<(RoadNode, f32)> {
        if !point.iter().all(|value| value.is_finite())
            || self.grid.is_empty()
            || !self
                .grid
                .contains_query_cell([query_cell(point[0]), query_cell(point[1])])
        {
            return self.nearest_node_linear(point);
        }

        let cell = [query_cell(point[0]), query_cell(point[1])];
        let mut best: Option<(RoadNode, f32)> = None;
        for ring in 0..=NEAREST_MAX_RINGS {
            let min_x = cell[0].saturating_sub(ring);
            let max_x = cell[0].saturating_add(ring);
            let min_z = cell[1].saturating_sub(ring);
            let max_z = cell[1].saturating_add(ring);
            if ring == 0 {
                self.scan_cell((cell[0], cell[1]), point, &mut best);
            } else {
                for x in min_x..=max_x {
                    self.scan_cell((x, min_z), point, &mut best);
                    self.scan_cell((x, max_z), point, &mut best);
                }
                for z in min_z.saturating_add(1)..max_z {
                    self.scan_cell((min_x, z), point, &mut best);
                    self.scan_cell((max_x, z), point, &mut best);
                }
            }
            let Some((_, best_distance)) = best else {
                continue;
            };
            if ring_proves_nearest(
                [min_x, min_z],
                [max_x, max_z],
                point,
                best_distance,
                self.grid.max_abs_coord,
            ) {
                return best;
            }
        }
        self.nearest_node_linear(point)
    }

    /// Exact linear nearest-node scan, retained as the reference.
    ///
    /// This scans every node and uses the same distance expression and
    /// tie-break as the indexed path. Tests and the traffic microbenchmark use
    /// it to prove the index returns identical nodes and distances.
    #[must_use]
    pub fn nearest_node_linear(&self, point: [f32; 2]) -> Option<(RoadNode, f32)> {
        let mut best: Option<(RoadNode, f32)> = None;
        for &node in &self.nodes {
            let distance = node_distance(node, point);
            if closer(node, distance, best) {
                best = Some((node, distance));
            }
        }
        best
    }

    fn scan_cell(&self, cell: (i32, i32), point: [f32; 2], best: &mut Option<(RoadNode, f32)>) {
        let Some(nodes) = self.grid.cells.get(&cell) else {
            return;
        };
        for &node in nodes {
            let distance = node_distance(node, point);
            if closer(node, distance, *best) {
                *best = Some((node, distance));
            }
        }
    }

    fn shortest_path(&self, start: RoadNode, goal: RoadNode) -> Option<u32> {
        if start == goal {
            return Some(0);
        }
        let mut distances: BTreeMap<RoadNode, u32> = BTreeMap::new();
        distances.insert(start, 0);
        let mut heap: BinaryHeap<Reverse<(u32, RoadNode)>> = BinaryHeap::new();
        heap.push(Reverse((0, start)));
        while let Some(Reverse((distance, node))) = heap.pop() {
            if node == goal {
                return Some(distance);
            }
            if distances.get(&node).is_some_and(|&known| distance > known) {
                continue;
            }
            let Some(neighbors) = self.adjacency.get(&node) else {
                continue;
            };
            for &(next, weight) in neighbors {
                let candidate = distance.saturating_add(weight);
                let known = distances.get(&next).copied();
                if known.is_none_or(|value| candidate < value) {
                    distances.insert(next, candidate);
                    heap.push(Reverse((candidate, next)));
                }
            }
        }
        None
    }

    /// Shortest-path node sequence from `start` to `goal`.
    ///
    /// Runs the full Dijkstra pass instead of stopping at the goal so equal
    /// distance alternatives are still comparable. An equal-length candidate
    /// replaces the predecessor only when its node sorts lower, which makes
    /// the reconstructed path independent of heap insertion order.
    fn path_nodes(&self, start: RoadNode, goal: RoadNode) -> Option<(u32, Vec<RoadNode>)> {
        if start == goal {
            return Some((0, vec![start]));
        }
        let mut distances: BTreeMap<RoadNode, u32> = BTreeMap::new();
        let mut previous: BTreeMap<RoadNode, RoadNode> = BTreeMap::new();
        distances.insert(start, 0);
        let mut heap: BinaryHeap<Reverse<(u32, RoadNode)>> = BinaryHeap::new();
        heap.push(Reverse((0, start)));
        while let Some(Reverse((distance, node))) = heap.pop() {
            if distances.get(&node).is_some_and(|&known| distance > known) {
                continue;
            }
            let Some(neighbors) = self.adjacency.get(&node) else {
                continue;
            };
            for &(next, weight) in neighbors {
                let candidate = distance.saturating_add(weight);
                let known = distances.get(&next).copied();
                let shorter = known.is_none_or(|value| candidate < value);
                let tie =
                    known == Some(candidate) && previous.get(&next).is_none_or(|&pred| node < pred);
                if shorter || tie {
                    distances.insert(next, candidate);
                    previous.insert(next, node);
                    heap.push(Reverse((candidate, next)));
                }
            }
        }

        let total = *distances.get(&goal)?;
        let mut path = vec![goal];
        let mut cursor = goal;
        while cursor != start {
            cursor = *previous.get(&cursor)?;
            path.push(cursor);
        }
        path.reverse();
        Some((total, path))
    }
}

fn push_unique_point(points: &mut Vec<[f32; 2]>, point: [f32; 2]) {
    if points.last() != Some(&point) {
        points.push(point);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn road(id: u64, start: [i32; 2], end: [i32; 2]) -> RoadSegment {
        RoadSegment {
            id,
            start,
            end,
            width_m: 8,
        }
    }

    #[test]
    fn an_empty_network_has_no_paths() {
        let graph = RoadGraph::build(&[]).expect("build");
        assert!(graph.is_empty());
        assert_eq!(graph.stats(), TrafficStats::default());
        assert_eq!(graph.distance_m([0.0, 0.0], [10.0, 0.0]), None);
        assert_eq!(graph.checksum(), RoadGraph::default().checksum());
    }

    #[test]
    fn a_single_segment_connects_its_endpoints() {
        let graph = RoadGraph::build(&[road(1, [0, 0], [20, 0])]).expect("build");
        let stats = graph.stats();
        assert_eq!(stats.roads, 1);
        assert_eq!(stats.nodes, 2);
        assert_eq!(stats.edges, 1);
        assert_eq!(stats.components, 1);
        assert!((stats.total_length_m - 20.0).abs() < 0.001);
        assert!(stats.connected());
        assert!((graph.distance_m([0.0, 0.0], [20.0, 0.0]).expect("path") - 20.0).abs() < 0.001);
        assert_eq!(graph.distance_m([5.0, 0.0], [5.0, 0.0]), Some(0.0));
    }

    #[test]
    fn an_l_shaped_network_routes_around_the_corner() {
        let graph = RoadGraph::build(&[road(1, [0, 0], [10, 0]), road(2, [10, 0], [10, 10])])
            .expect("build");
        let distance = graph.distance_m([0.0, 0.0], [10.0, 10.0]).expect("path");
        assert!((distance - 20.0).abs() < 0.001);
        assert!(graph.distance_m([0.0, 0.0], [10.0, 10.0]).expect("path") < 21.0);
    }

    #[test]
    fn crossing_segments_split_at_the_intersection() {
        let graph = RoadGraph::build(&[road(1, [0, 5], [20, 5]), road(2, [10, 0], [10, 20])])
            .expect("build");
        let stats = graph.stats();
        assert_eq!(stats.nodes, 5);
        assert_eq!(stats.edges, 4);
        assert_eq!(stats.components, 1);
        assert!((graph.distance_m([0.0, 5.0], [10.0, 20.0]).expect("path") - 25.0).abs() < 0.001);
    }

    #[test]
    fn a_t_junction_endpoint_splits_the_through_road() {
        let graph = RoadGraph::build(&[road(1, [0, 0], [20, 0]), road(2, [10, 0], [10, 10])])
            .expect("build");
        let stats = graph.stats();
        assert_eq!(stats.nodes, 4);
        assert_eq!(stats.edges, 3);
        assert!((graph.distance_m([0.0, 0.0], [10.0, 10.0]).expect("path") - 20.0).abs() < 0.001);
    }

    #[test]
    fn disconnected_components_have_no_path() {
        let graph = RoadGraph::build(&[road(1, [0, 0], [10, 0]), road(2, [50, 50], [60, 50])])
            .expect("build");
        let stats = graph.stats();
        assert_eq!(stats.components, 2);
        assert!(!stats.connected());
        assert_eq!(graph.distance_m([0.0, 0.0], [50.0, 50.0]), None);
    }

    #[test]
    fn off_network_points_include_walking_legs() {
        let graph = RoadGraph::build(&[road(1, [0, 0], [20, 0])]).expect("build");
        let distance = graph.distance_m([-3.0, 4.0], [23.0, 4.0]).expect("path");
        assert!((distance - 30.0).abs() < 0.001);
    }

    #[test]
    fn route_points_follow_a_straight_segment() {
        let graph = RoadGraph::build(&[road(1, [0, 0], [20, 0])]).expect("build");
        let points = graph.route_points([0.0, 0.0], [20.0, 0.0]).expect("route");
        assert_eq!(points, vec![[0.0, 0.0], [20.0, 0.0]]);
    }

    #[test]
    fn route_points_turn_at_an_l_corner() {
        let graph = RoadGraph::build(&[road(1, [0, 0], [10, 0]), road(2, [10, 0], [10, 10])])
            .expect("build");
        let points = graph.route_points([0.0, 0.0], [10.0, 10.0]).expect("route");
        assert_eq!(points, vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]);
    }

    #[test]
    fn route_points_include_both_walking_legs() {
        let graph = RoadGraph::build(&[road(1, [0, 0], [20, 0])]).expect("build");
        let points = graph.route_points([-3.0, 4.0], [23.0, 4.0]).expect("route");
        assert_eq!(
            points,
            vec![[-3.0, 4.0], [0.0, 0.0], [20.0, 0.0], [23.0, 4.0]]
        );
        let length: f32 = points
            .windows(2)
            .map(|pair| {
                let dx = pair[1][0] - pair[0][0];
                let dz = pair[1][1] - pair[0][1];
                (dx * dx + dz * dz).sqrt()
            })
            .sum();
        assert!((length - 30.0).abs() < 0.001);
    }

    #[test]
    fn route_points_reject_invalid_or_disconnected_pairs() {
        let graph = RoadGraph::build(&[road(1, [0, 0], [10, 0]), road(2, [50, 50], [60, 50])])
            .expect("build");
        assert_eq!(graph.route_points([0.0, 0.0], [50.0, 50.0]), None);
        assert_eq!(graph.route_points([f32::NAN, 0.0], [0.0, 0.0]), None);
        assert_eq!(
            RoadGraph::build(&[])
                .expect("build")
                .route_points([0.0, 0.0], [10.0, 0.0]),
            None
        );
        assert_eq!(
            RoadGraph::build(&[])
                .expect("build")
                .route_points([5.0, 5.0], [5.0, 5.0]),
            Some(vec![[5.0, 5.0], [5.0, 5.0]])
        );
    }

    #[test]
    fn route_points_break_equal_length_ties_deterministically() {
        let roads = [
            road(1, [0, 0], [10, 0]),
            road(2, [10, 0], [10, 10]),
            road(3, [0, 0], [0, 10]),
            road(4, [0, 10], [10, 10]),
        ];
        let first = RoadGraph::build(&roads).expect("build");
        let second = RoadGraph::build(&roads).expect("build");
        let expected = vec![[0.0, 0.0], [0.0, 10.0], [10.0, 10.0]];
        assert_eq!(
            first.route_points([0.0, 0.0], [10.0, 10.0]),
            Some(expected.clone())
        );
        assert_eq!(
            second.route_points([0.0, 0.0], [10.0, 10.0]),
            Some(expected)
        );
    }

    #[test]
    fn duplicate_and_overlapping_edges_are_deduplicated() {
        let graph = RoadGraph::build(&[
            road(1, [0, 0], [10, 0]),
            road(2, [0, 0], [10, 0]),
            road(3, [5, 0], [15, 0]),
        ])
        .expect("build");
        let stats = graph.stats();
        assert_eq!(stats.nodes, 4);
        assert_eq!(stats.edges, 3);
        assert!((stats.total_length_m - 15.0).abs() < 0.001);
        let distance = graph.distance_m([0.0, 0.0], [15.0, 0.0]).expect("path");
        assert!((distance - 15.0).abs() < 0.001);
    }

    #[test]
    fn non_finite_or_unjointed_points_are_rejected() {
        let graph = RoadGraph::build(&[road(1, [0, 0], [10, 0])]).expect("build");
        assert_eq!(graph.distance_m([f32::NAN, 0.0], [0.0, 0.0]), None);
        assert_eq!(
            RoadGraph::build(&[road(1, [0, 0], [0, 0])])
                .expect("build")
                .distance_m([0.0, 0.0], [1.0, 0.0]),
            None
        );
    }

    #[test]
    fn a_flow_field_answers_many_origins() {
        let graph = RoadGraph::build(&[
            road(1, [0, 0], [10, 0]),
            road(2, [10, 0], [10, 10]),
            road(3, [50, 50], [60, 50]),
        ])
        .expect("build");
        let field = graph.flow_to([10.0, 10.0]).expect("field");
        assert_eq!(field.reachable_nodes(), 3);
        assert!(
            (graph.flow_distance_m([0.0, 0.0], &field).expect("distance") - 20.0).abs() < 0.001
        );
        assert!(
            (graph
                .flow_distance_m([10.0, 0.0], &field)
                .expect("distance")
                - 10.0)
                .abs()
                < 0.001
        );
        assert_eq!(graph.flow_distance_m([10.0, 10.0], &field), Some(0.0));

        assert_eq!(graph.flow_distance_m([50.0, 50.0], &field), None);
        assert!(graph.flow_to([50.0, 50.0]).is_some());
    }

    #[test]
    fn a_flow_field_cache_reuses_repeated_goals() {
        let graph = RoadGraph::build(&[road(1, [0, 0], [10, 0]), road(2, [10, 0], [10, 10])])
            .expect("build");
        let goal = [10.0, 10.0];
        let direct = graph.flow_to(goal).expect("field");

        let mut cache = FlowFieldCache::new(4);
        let first = cache
            .get_or_build(&graph, goal)
            .expect("cached field")
            .checksum();
        let second = cache
            .get_or_build(&graph, goal)
            .expect("cached field")
            .checksum();
        assert_eq!(first, direct.checksum());
        assert_eq!(second, first);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.capacity(), 4);
        assert_eq!(cache.hits(), 1);
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.rebuilds(), 0);
        assert!(cache.get(goal).is_some());
        assert!(cache.get_for(&graph, goal).is_some());

        cache.clear();
        assert!(cache.is_empty());
        assert!(cache.get(goal).is_none());
        assert_eq!(cache.hits(), 1);
        assert_eq!(cache.rebuilds(), 0);
    }

    #[test]
    fn a_full_flow_field_cache_evicts_the_oldest_goal() {
        let graph = RoadGraph::build(&[
            road(1, [0, 0], [10, 0]),
            road(2, [10, 0], [10, 10]),
            road(3, [10, 10], [0, 10]),
        ])
        .expect("build");
        let goals = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]];
        let mut cache = FlowFieldCache::new(2);
        for goal in goals {
            assert!(cache.get_or_build(&graph, goal).is_some());
        }
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.misses(), 3);
        assert_eq!(cache.hits(), 0);
        assert!(cache.get(goals[0]).is_none());
        assert!(cache.get(goals[1]).is_some());
        assert!(cache.get(goals[2]).is_some());

        assert!(cache.get_or_build(&graph, goals[0]).is_some());
        assert_eq!(cache.hits(), 0);
        assert_eq!(cache.misses(), 4);
        assert!(cache.get(goals[0]).is_some());
        assert!(cache.get(goals[1]).is_none());
        assert!(cache.get(goals[2]).is_some());
    }

    #[test]
    fn a_flow_field_cache_does_not_store_invalid_goals() {
        let graph = RoadGraph::build(&[road(1, [0, 0], [10, 0])]).expect("build");
        let mut cache = FlowFieldCache::new(2);
        assert!(cache.get_or_build(&graph, [f32::NAN, 0.0]).is_none());
        assert!(cache.get_or_build(&graph, [0.0, f32::INFINITY]).is_none());
        assert!(cache.is_empty());
        assert_eq!(cache.misses(), 2);
        assert_eq!(cache.hits(), 0);
        assert!(cache.get([f32::NAN, 0.0]).is_none());
    }

    #[test]
    fn flow_field_checksums_are_stable_and_goal_specific() {
        let roads = [road(1, [0, 0], [10, 0]), road(2, [10, 0], [10, 10])];
        let first = RoadGraph::build(&roads).expect("build");
        let second = RoadGraph::build(&roads).expect("build");
        let from_corner = first.flow_to([10.0, 10.0]).expect("field");
        let rebuilt = second.flow_to([10.0, 10.0]).expect("field");
        let from_origin = first.flow_to([0.0, 0.0]).expect("field");
        assert_eq!(from_corner.checksum(), rebuilt.checksum());
        assert_ne!(from_corner.checksum(), from_origin.checksum());
    }

    #[test]
    fn graph_change_version_is_stable_and_tracks_appends() {
        let roads = [road(1, [0, 0], [10, 0]), road(2, [10, 0], [10, 10])];
        let mut graph = RoadGraph::build(&roads).expect("build");
        let identical = RoadGraph::build(&roads).expect("build");
        assert_eq!(graph.change_version(), identical.change_version());
        assert_eq!(graph.change_version(), graph.clone().change_version());
        let base = graph.change_version();

        // Duplicate geometry under a new id adds a road record but no graph
        // structure, so the version must not move.
        assert!(!graph
            .add_segment(&road(3, [0, 0], [10, 0]))
            .expect("duplicate"));
        assert_eq!(graph.change_version(), base);

        assert!(graph
            .add_segment(&road(4, [10, 10], [10, 20]))
            .expect("append"));
        assert_ne!(graph.change_version(), base);

        // A fresh build has the same content checksum but a new lineage
        // token: the cache rebuilds once instead of trusting an equal-looking
        // graph. That is the conservative half of the contract.
        let rebuilt = RoadGraph::build(&[
            road(1, [0, 0], [10, 0]),
            road(2, [10, 0], [10, 10]),
            road(3, [0, 0], [10, 0]),
            road(4, [10, 10], [10, 20]),
        ])
        .expect("build");
        assert_eq!(graph.checksum(), rebuilt.checksum());
        assert_ne!(graph.change_version(), rebuilt.change_version());
    }

    #[test]
    fn adding_a_road_rebuilds_the_cached_field() {
        let roads = [
            road(1, [0, 0], [64, 0]),
            road(2, [64, 0], [64, 64]),
            road(3, [128, 0], [192, 0]),
        ];
        let mut graph = RoadGraph::build(&roads).expect("build");
        let goal = [64.0, 64.0];
        let mut cache = FlowFieldCache::new(4);

        let base_field = cache.get_or_build(&graph, goal).expect("base field");
        let base_checksum = base_field.checksum();
        assert_eq!(base_field.reachable_nodes(), 3);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.hits(), 0);
        assert_eq!(cache.rebuilds(), 0);
        assert!(cache.get_for(&graph, goal).is_some());

        // Connect the disconnected component. The goal can now reach the
        // added road's endpoints, so the old field is provably stale.
        assert!(graph
            .add_segment(&road(4, [64, 0], [128, 0]))
            .expect("append"));
        assert!(cache.get_for(&graph, goal).is_none());
        let expected_field = graph.flow_to(goal).expect("fresh field");
        let expected = expected_field.checksum();
        assert_eq!(expected_field.reachable_nodes(), 5);
        assert_ne!(expected, base_checksum);

        let rebuilt = cache
            .get_or_build(&graph, goal)
            .expect("rebuilt field")
            .checksum();
        assert_eq!(rebuilt, expected);
        assert_eq!(cache.len(), 1, "stale entry is replaced in place");
        assert_eq!(cache.misses(), 2);
        assert_eq!(cache.hits(), 0);
        assert_eq!(cache.rebuilds(), 1);
        assert_eq!(cache.get(goal).expect("raw entry").checksum(), expected);

        // The rebuilt entry is current: a second query is a plain hit.
        assert_eq!(
            cache
                .get_or_build(&graph, goal)
                .expect("cached field")
                .checksum(),
            expected
        );
        assert_eq!(cache.hits(), 1);
        assert_eq!(cache.rebuilds(), 1);
    }

    #[test]
    fn a_different_graph_with_the_same_goal_rebuilds() {
        let first = RoadGraph::build(&[road(1, [0, 0], [10, 0])]).expect("build");
        let second = RoadGraph::build(&[road(1, [0, 0], [10, 0]), road(2, [10, 0], [10, 10])])
            .expect("build");
        let goal = [10.0, 0.0];
        let mut cache = FlowFieldCache::new(2);

        let first_checksum = cache
            .get_or_build(&first, goal)
            .expect("first field")
            .checksum();
        assert_eq!(
            first_checksum,
            first.flow_to(goal).expect("fresh first").checksum()
        );

        let second_checksum = cache
            .get_or_build(&second, goal)
            .expect("second field")
            .checksum();
        assert_eq!(
            second_checksum,
            second.flow_to(goal).expect("fresh second").checksum()
        );
        assert_ne!(second_checksum, first_checksum);
        assert_eq!(cache.hits(), 0);
        assert_eq!(cache.misses(), 2);
        assert_eq!(cache.rebuilds(), 1);

        // Raw `get` returns the current entry; `get_for` refuses the old
        // graph's version.
        assert!(cache.get(goal).is_some());
        assert!(cache.get_for(&second, goal).is_some());
        assert!(cache.get_for(&first, goal).is_none());
    }

    #[test]
    fn invalidate_drops_fields_and_keeps_counters() {
        let graph = RoadGraph::build(&[road(1, [0, 0], [10, 0])]).expect("build");
        let goal = [10.0, 0.0];
        let mut cache = FlowFieldCache::new(2);
        assert!(cache.get_or_build(&graph, goal).is_some());
        assert!(cache.get_or_build(&graph, goal).is_some());
        assert_eq!(cache.hits(), 1);
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.rebuilds(), 0);

        cache.invalidate();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
        assert!(cache.get(goal).is_none());
        assert!(cache.get_for(&graph, goal).is_none());
        assert_eq!(cache.hits(), 1);
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.rebuilds(), 0);
        assert_eq!(cache.capacity(), 2);

        // After invalidation the next lookup is a fresh build, not a rebuild.
        assert!(cache.get_or_build(&graph, goal).is_some());
        assert_eq!(cache.misses(), 2);
        assert_eq!(cache.rebuilds(), 0);
    }

    #[test]
    fn a_rebuild_that_cannot_build_drops_the_stale_entry() {
        let graph = RoadGraph::build(&[road(1, [0, 0], [10, 0])]).expect("build");
        let empty = RoadGraph::build(&[]).expect("build");
        let goal = [10.0, 0.0];
        let mut cache = FlowFieldCache::new(2);
        assert!(cache.get_or_build(&graph, goal).is_some());

        assert!(cache.get_or_build(&empty, goal).is_none());
        assert_eq!(cache.misses(), 2);
        assert_eq!(cache.rebuilds(), 1);
        assert!(cache.is_empty());
        assert!(cache.get(goal).is_none());
    }

    #[test]
    fn flow_cache_versions_and_counters_are_deterministic() {
        fn run() -> (u64, u64, u64, u64, Vec<u64>) {
            let roads = [
                road(1, [0, 0], [10, 0]),
                road(2, [10, 0], [10, 10]),
                road(3, [0, 0], [0, 10]),
                road(4, [0, 10], [10, 10]),
            ];
            let mut graph = RoadGraph::build(&roads).expect("build");
            let goals = [[10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
            let mut cache = FlowFieldCache::new(4);
            let mut checksums = Vec::new();
            for goal in goals {
                checksums.push(
                    cache
                        .get_or_build(&graph, goal)
                        .expect("cached field")
                        .checksum(),
                );
            }
            assert!(graph
                .add_segment(&road(5, [10, 0], [20, 0]))
                .expect("append"));
            for goal in goals {
                checksums.push(
                    cache
                        .get_or_build(&graph, goal)
                        .expect("rebuilt field")
                        .checksum(),
                );
            }
            (
                graph.change_version(),
                cache.hits(),
                cache.misses(),
                cache.rebuilds(),
                checksums,
            )
        }

        let first = run();
        let second = run();
        assert_eq!(first, second);
        assert_eq!(first.1, 0, "every goal is rebuilt after the append");
        assert_eq!(first.2, 6);
        assert_eq!(first.3, 3);
    }

    #[test]
    fn travel_time_is_distance_over_speed() {
        assert_eq!(
            travel_time_seconds(0.0, DEFAULT_TRAVEL_SPEED_MPS),
            Some(0.0)
        );
        assert_eq!(
            travel_time_seconds(80.0, DEFAULT_TRAVEL_SPEED_MPS),
            Some(10.0)
        );
        let default = travel_time_seconds(136.4, DEFAULT_TRAVEL_SPEED_MPS).expect("time");
        assert!((default - 17.05).abs() < 0.001);
        let custom = travel_time_seconds(100.0, 4.0).expect("time");
        assert!((custom - 25.0).abs() < 0.001);
        // Negative zero distance is non-negative and canonicalizes to zero.
        assert_eq!(travel_time_seconds(-0.0, 8.0), Some(0.0));
    }

    #[test]
    fn invalid_travel_time_inputs_are_rejected() {
        assert_eq!(travel_time_seconds(-1.0, 8.0), None);
        assert_eq!(travel_time_seconds(f32::NAN, 8.0), None);
        assert_eq!(travel_time_seconds(f32::INFINITY, 8.0), None);
        assert_eq!(travel_time_seconds(f32::NEG_INFINITY, 8.0), None);
        assert_eq!(travel_time_seconds(10.0, 0.0), None);
        assert_eq!(travel_time_seconds(10.0, -8.0), None);
        assert_eq!(travel_time_seconds(10.0, f32::NAN), None);
        assert_eq!(travel_time_seconds(10.0, f32::INFINITY), None);
        // A finite result that overflows to infinity is rejected.
        assert_eq!(
            travel_time_seconds(f32::MAX, f32::MIN_POSITIVE),
            None,
            "overflow must not produce an infinite travel time"
        );
    }

    #[test]
    fn checksum_is_stable_and_changes_with_geometry() {
        let first = RoadGraph::build(&[road(1, [0, 0], [10, 0])]).expect("build");
        let second = RoadGraph::build(&[road(1, [0, 0], [10, 0])]).expect("build");
        let third = RoadGraph::build(&[road(1, [0, 0], [12, 0])]).expect("build");
        assert_eq!(first.checksum(), second.checksum());
        assert_ne!(first.checksum(), third.checksum());
    }

    fn grid_roads(size: usize, spacing: i32, origin: [i32; 2]) -> Vec<RoadSegment> {
        assert!(size >= 2);
        let extent = (size as i32 - 1) * spacing;
        let mut roads = Vec::with_capacity(size * 2);
        let mut id = 1_u64;
        for index in 0..size {
            let offset = index as i32 * spacing;
            roads.push(road(
                id,
                [origin[0], origin[1] + offset],
                [origin[0] + extent, origin[1] + offset],
            ));
            id += 1;
            roads.push(road(
                id,
                [origin[0] + offset, origin[1]],
                [origin[0] + offset, origin[1] + extent],
            ));
            id += 1;
        }
        roads
    }

    fn next_test_coord(seed: &mut u64, min: i32, max: i32) -> f32 {
        *seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let quarter_steps = u64::try_from((max - min) * 4).expect("range");
        min as f32 + ((*seed >> 33) % (quarter_steps + 1)) as f32 * 0.25
    }

    fn assert_matches_linear(graph: &RoadGraph, points: &[[f32; 2]]) {
        for &point in points {
            assert_eq!(
                graph.nearest_node(point),
                graph.nearest_node_linear(point),
                "indexed lookup disagreed at {point:?}"
            );
        }
    }

    #[test]
    fn indexed_nearest_matches_linear_for_every_node_and_nearby_point() {
        let graph = RoadGraph::build(&grid_roads(12, 16, [-96, -96])).expect("build");
        let mut points = Vec::with_capacity(graph.nodes.len() * 7);
        for &node in &graph.nodes {
            let x = node[0] as f32;
            let z = node[1] as f32;
            points.push([x, z]);
            points.push([x + 0.5, z]);
            points.push([x, z + 0.5]);
            points.push([x - 3.25, z + 7.5]);
            points.push([x + 8.5, z + 8.5]);
        }
        assert_matches_linear(&graph, &points);
    }

    #[test]
    fn indexed_nearest_matches_linear_at_road_midpoints() {
        let graph = RoadGraph::build(&grid_roads(16, 16, [0, 0])).expect("build");
        let mut points = Vec::with_capacity(graph.nodes.len() * 3);
        for &node in &graph.nodes {
            let x = node[0] as f32;
            let z = node[1] as f32;
            points.push([x + 8.0, z]);
            points.push([x, z + 8.0]);
            points.push([x + 8.0, z + 8.0]);
        }
        assert_matches_linear(&graph, &points);
    }

    #[test]
    fn indexed_nearest_matches_linear_across_and_outside_the_bounds() {
        let graph = RoadGraph::build(&grid_roads(20, 24, [-240, -240])).expect("build");
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut points = Vec::with_capacity(2_000);
        for _ in 0..2_000 {
            points.push([
                next_test_coord(&mut seed, -480, 480),
                next_test_coord(&mut seed, -480, 480),
            ]);
        }
        assert_matches_linear(&graph, &points);
    }

    #[test]
    fn far_away_queries_fall_back_to_the_linear_scan() {
        let graph = RoadGraph::build(&grid_roads(8, 16, [0, 0])).expect("build");
        // The grid ends at 112 m. One cell beyond still uses the index; two
        // cells beyond and every distant point use the exact linear scan.
        assert_matches_linear(
            &graph,
            &[
                [144.0, 0.0],
                [144.0, 144.0],
                [160.0, 0.0],
                [1_000_000.0, -1_000_000.0],
                [-1_000_000.0, 1_000_000.0],
                [1.0e9, 1.0e9],
                [-3.0e8, 0.0],
                [0.0, 5.0e8],
            ],
        );
    }

    #[test]
    fn an_empty_graph_has_no_nearest_node() {
        let graph = RoadGraph::build(&[]).expect("build");
        assert_eq!(graph.nearest_node([0.0, 0.0]), None);
        assert_eq!(graph.nearest_node_linear([0.0, 0.0]), None);
        assert_eq!(graph.nearest_node([f32::NAN, 0.0]), None);
    }

    #[test]
    fn equal_distance_ties_keep_the_stable_node_choice() {
        let plus = RoadGraph::build(&[
            road(1, [-10, 0], [-20, 0]),
            road(2, [10, 0], [20, 0]),
            road(3, [0, -10], [0, -20]),
            road(4, [0, 10], [0, 20]),
        ])
        .expect("build");
        let indexed_result = plus.nearest_node([0.0, 0.0]);
        assert_eq!(indexed_result, plus.nearest_node_linear([0.0, 0.0]));
        let indexed = indexed_result.expect("nearest");
        // Four nodes are exactly 10 m away; the lexicographically smallest wins.
        assert_eq!(indexed, ([-10, 0], 10.0));

        let square = RoadGraph::build(&[
            road(11, [0, 0], [10, 0]),
            road(12, [10, 0], [10, 10]),
            road(13, [10, 10], [0, 10]),
            road(14, [0, 10], [0, 0]),
        ])
        .expect("build");
        let indexed_result = square.nearest_node([5.0, 5.0]);
        assert_eq!(indexed_result, square.nearest_node_linear([5.0, 5.0]));
        assert_eq!(indexed_result.expect("nearest").0, [0, 0]);
    }

    #[test]
    fn sparse_graphs_fall_back_after_the_ring_cap() {
        let graph = RoadGraph::build(&[
            road(1, [0, 0], [10, 0]),
            road(2, [10_000, 10_000], [10_010, 10_000]),
        ])
        .expect("build");
        assert_matches_linear(
            &graph,
            &[
                [0.0, 0.0],
                [9_900.0, 9_900.0],
                [5_000.0, 5_000.0],
                [5_000.0, 5.0],
            ],
        );
    }

    #[test]
    fn non_finite_queries_use_the_linear_reference() {
        let graph = RoadGraph::build(&grid_roads(4, 16, [0, 0])).expect("build");
        for point in [
            [f32::NAN, 0.0],
            [0.0, f32::NAN],
            [f32::INFINITY, f32::INFINITY],
            [f32::NEG_INFINITY, 0.0],
        ] {
            let indexed = graph.nearest_node(point);
            let linear = graph.nearest_node_linear(point);
            match (indexed, linear) {
                (Some((indexed_node, indexed_distance)), Some((linear_node, linear_distance))) => {
                    assert_eq!(indexed_node, linear_node);
                    assert_eq!(indexed_distance.to_bits(), linear_distance.to_bits());
                }
                (None, None) => {}
                other => panic!("indexed and linear disagreed at {point:?}: {other:?}"),
            }
        }
    }

    #[test]
    fn indexed_nearest_matches_linear_on_a_large_grid() {
        let graph = RoadGraph::build(&grid_roads(64, 16, [0, 0])).expect("build");
        assert_eq!(graph.nodes.len(), 4_096);
        let mut points = Vec::with_capacity(graph.nodes.len() * 2 + 256);
        for &node in &graph.nodes {
            points.push([node[0] as f32, node[1] as f32]);
            points.push([node[0] as f32 + 8.0, node[1] as f32 + 3.5]);
        }
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        for _ in 0..256 {
            points.push([
                next_test_coord(&mut seed, -32, 1_040),
                next_test_coord(&mut seed, -32, 1_040),
            ]);
        }
        assert_matches_linear(&graph, &points);
    }

    /// Assert that an appended graph is indistinguishable from a fresh build:
    /// checksum, statistics, node list, every nearest-node lookup, and every
    /// ordered node-pair route and distance.
    fn assert_equivalent(incremental: &RoadGraph, fresh: &RoadGraph) {
        assert_eq!(incremental.checksum(), fresh.checksum(), "checksum");
        assert_eq!(incremental.stats(), fresh.stats(), "stats");
        assert_eq!(incremental.nodes, fresh.nodes, "nodes");
        for &node in &fresh.nodes {
            let point = [node[0] as f32, node[1] as f32];
            assert_eq!(
                incremental.nearest_node(point),
                fresh.nearest_node(point),
                "nearest at {point:?}"
            );
            assert_eq!(
                incremental.nearest_node_linear(point),
                fresh.nearest_node_linear(point),
                "linear nearest at {point:?}"
            );
        }
        for &from_node in &fresh.nodes {
            for &to_node in &fresh.nodes {
                let from = [from_node[0] as f32, from_node[1] as f32];
                let to = [to_node[0] as f32, to_node[1] as f32];
                assert_eq!(
                    incremental.distance_m(from, to).map(f32::to_bits),
                    fresh.distance_m(from, to).map(f32::to_bits),
                    "distance {from:?} -> {to:?}"
                );
                assert_eq!(
                    incremental.route_points(from, to),
                    fresh.route_points(from, to),
                    "route {from:?} -> {to:?}"
                );
            }
        }
    }

    #[test]
    fn appending_one_segment_to_an_empty_graph_matches_build() {
        let segment = road(7, [0, 0], [20, 0]);
        let mut graph = RoadGraph::build(&[]).expect("build");
        assert!(graph.add_segment(&segment).expect("append"));
        let fresh = RoadGraph::build(&[segment]).expect("build");
        assert_equivalent(&graph, &fresh);
        assert_eq!(graph.stats().roads, 1);
    }

    #[test]
    fn appending_a_grid_matches_a_fresh_build() {
        let roads = grid_roads(6, 16, [-40, -40]);
        let mut incremental = RoadGraph::build(&[]).expect("build");
        for (index, segment) in roads.iter().enumerate() {
            assert!(
                incremental.add_segment(segment).expect("append"),
                "segment {index} should be new"
            );
        }
        let fresh = RoadGraph::build(&roads).expect("build");
        assert_equivalent(&incremental, &fresh);
        assert_eq!(incremental.stats().components, 1);
    }

    #[test]
    fn appending_collinear_and_crossing_segments_matches_build() {
        let roads = [
            road(1, [0, 0], [20, 0]),
            road(2, [5, 0], [15, 0]),
            road(3, [10, 0], [10, 10]),
            road(4, [0, 5], [20, 5]),
            road(5, [10, 5], [10, 15]),
            road(6, [20, 0], [20, 20]),
            road(7, [-10, 10], [30, 10]),
        ];
        let mut incremental = RoadGraph::build(&[]).expect("build");
        for (index, segment) in roads.iter().enumerate() {
            incremental.add_segment(segment).expect("append");
            let fresh = RoadGraph::build(&roads[..=index]).expect("build");
            assert_equivalent(&incremental, &fresh);
        }
    }

    #[test]
    fn duplicate_appends_are_no_ops() {
        let first = road(1, [0, 0], [10, 0]);
        let mut graph = RoadGraph::build(&[]).expect("build");
        assert!(graph.add_segment(&first).expect("append"));
        let checksum = graph.checksum();
        let stats = graph.stats();

        // Exact id and geometry: no graph change and no extra record.
        assert!(!graph.add_segment(&first).expect("duplicate"));
        assert_eq!(graph.checksum(), checksum);
        assert_eq!(graph.stats(), stats);

        // Same geometry under a new id: the graph is unchanged, but the
        // record counts exactly as `build` would count it.
        let duplicate_geometry = road(2, [0, 0], [10, 0]);
        assert!(!graph.add_segment(&duplicate_geometry).expect("duplicate"));
        assert_eq!(graph.checksum(), checksum);
        assert_eq!(graph.stats().roads, 2);

        let fresh = RoadGraph::build(&[first, duplicate_geometry]).expect("build");
        assert_eq!(graph.checksum(), fresh.checksum());
        assert_eq!(graph.stats(), fresh.stats());
    }

    #[test]
    fn invalid_appends_leave_the_graph_unchanged() {
        let mut graph = RoadGraph::build(&[road(1, [0, 0], [10, 0]), road(2, [10, 0], [10, 10])])
            .expect("build");
        let before = graph.clone();
        for (segment, reason) in [
            (road(99, [0, 0], [0, 0]), "zero_length"),
            (road(99, [0, 0], [5, 5]), "not_axis_aligned"),
            (
                road(99, [1_000_001, 0], [1_000_010, 0]),
                "coordinate_out_of_range",
            ),
            (
                road(99, [-1_000_001, 0], [-1_000_010, 0]),
                "coordinate_out_of_range",
            ),
        ] {
            assert_eq!(
                graph.add_segment(&segment),
                Err(TrafficError::InvalidSegment { id: 99, reason }),
                "segment {segment:?}"
            );
            assert_eq!(graph, before, "graph changed after rejecting {segment:?}");
        }

        let conflicting_id = road(1, [0, 0], [5, 0]);
        assert_eq!(
            graph.add_segment(&conflicting_id),
            Err(TrafficError::DuplicateRoadId(1))
        );
        assert_eq!(graph, before);
    }

    #[test]
    fn appending_respects_the_road_cap() {
        let mut graph = RoadGraph::build(&[]).expect("build");
        // Poke the private counter instead of appending 10,000 roads.
        graph.road_count = MAX_TRAFFIC_ROADS;
        assert_eq!(
            graph.add_segment(&road(1, [0, 0], [10, 0])),
            Err(TrafficError::TooManyRoads(MAX_TRAFFIC_ROADS + 1))
        );
        assert!(graph.is_empty());
        assert_eq!(graph.stats().roads, MAX_TRAFFIC_ROADS);
    }

    #[test]
    fn appending_respects_the_node_cap() {
        let mut graph = RoadGraph::build(&[road(1, [0, 0], [10, 0])]).expect("build");
        for index in 0..MAX_TRAFFIC_NODES {
            graph
                .adjacency
                .insert([1_000 + index as i32, 0], Vec::new());
        }
        let node_count = graph.adjacency.len();
        assert_eq!(
            graph.add_segment(&road(2, [0, 0], [10, 0])),
            Err(TrafficError::TooManyNodes(MAX_TRAFFIC_NODES + 2))
        );
        assert_eq!(graph.adjacency.len(), node_count);
        assert_eq!(graph.stats().roads, 1);
    }

    #[test]
    fn appending_a_segment_connects_two_components() {
        let first = road(1, [0, 0], [10, 0]);
        let second = road(2, [20, 0], [30, 0]);
        let mut graph = RoadGraph::build(&[first.clone(), second.clone()]).expect("build");
        assert_eq!(graph.stats().components, 2);
        assert_eq!(graph.distance_m([0.0, 0.0], [30.0, 0.0]), None);

        let bridge = road(3, [10, 0], [20, 0]);
        assert!(graph.add_segment(&bridge).expect("append"));
        assert_eq!(graph.stats().components, 1);
        assert!((graph.distance_m([0.0, 0.0], [30.0, 0.0]).expect("path") - 30.0).abs() < 0.001);
        assert_equivalent(
            &graph,
            &RoadGraph::build(&[first, second, bridge]).expect("build"),
        );
    }

    #[test]
    fn extend_segments_counts_new_geometry_and_stops_on_error() {
        let first = road(1, [0, 0], [10, 0]);
        let second = road(2, [10, 0], [10, 10]);
        let mut graph = RoadGraph::build(&[]).expect("build");
        let result =
            graph.extend_segments(&[first.clone(), road(3, [0, 0], [0, 0]), second.clone()]);
        assert_eq!(
            result,
            Err(TrafficError::InvalidSegment {
                id: 3,
                reason: "zero_length"
            })
        );
        // The first segment landed; the invalid segment stopped the batch.
        assert_eq!(graph.stats().roads, 1);
        assert_eq!(
            graph.checksum(),
            RoadGraph::build(std::slice::from_ref(&first))
                .expect("build")
                .checksum()
        );
        assert!(graph.add_segment(&second).expect("append"));
        assert_eq!(
            graph.checksum(),
            RoadGraph::build(&[first, second])
                .expect("build")
                .checksum()
        );
    }

    #[test]
    fn extend_segments_does_not_count_already_covered_geometry() {
        let first = road(1, [0, 0], [10, 0]);
        let covered = road(2, [2, 0], [8, 0]);
        let mut graph = RoadGraph::build(&[]).expect("build");
        assert_eq!(
            graph
                .extend_segments(&[first.clone(), covered.clone()])
                .expect("extend"),
            1
        );
        assert_eq!(graph.stats().roads, 2);
        assert_eq!(
            graph.checksum(),
            RoadGraph::build(&[first, covered])
                .expect("build")
                .checksum()
        );
    }
}
