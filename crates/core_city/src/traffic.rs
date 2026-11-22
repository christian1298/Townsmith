//! Derived road graph and road-distance commute support.
//!
//! Roads are saved as straight, axis-aligned segments. This module turns them
//! into a graph at segment endpoints, T-junctions, and crossings so the city
//! can measure travel along the network instead of straight-line distance. It
//! is derived state: nothing here is persisted or rendered.
//!
//! The graph is deliberately small and deterministic. It splits every segment
//! at crossings and endpoints, deduplicates shared edges, and runs Dijkstra
//! with integer metre weights. A spatial index is future work before the
//! hundred-thousand-building scale.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, VecDeque};

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

/// FNV-1a 64-bit offset basis used by graph, field, and route checksums.
pub(crate) const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

/// FNV-1a 64-bit prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

pub(crate) fn fnv1a_feed(hash: &mut u64, value: u64) {
    *hash ^= value;
    *hash = hash.wrapping_mul(FNV_PRIME);
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

/// Errors returned while building a traffic graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrafficError {
    /// Too many road segments for one graph.
    TooManyRoads(usize),
    /// Too many split nodes for one graph.
    TooManyNodes(usize),
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
        }
    }
}

impl std::error::Error for TrafficError {}

/// A derived graph over the saved road network.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RoadGraph {
    adjacency: BTreeMap<RoadNode, Vec<(RoadNode, u32)>>,
    nodes: Vec<RoadNode>,
    road_count: usize,
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
/// The cache holds no road-edit version. A road edit rebuilds the graph, so
/// callers must drop the cache (or call [`Self::clear`]) when the roads change.
/// [`RoadGraph`] itself remains immutable, and its public behavior is
/// unchanged.
#[derive(Debug, Clone)]
pub struct FlowFieldCache {
    fields: BTreeMap<[u32; 2], FlowField>,
    order: VecDeque<[u32; 2]>,
    capacity: usize,
    hits: u64,
    misses: u64,
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
        }
    }

    /// Return a cached field for an exact goal point, if present.
    #[must_use]
    pub fn get(&self, goal: [f32; 2]) -> Option<&FlowField> {
        self.fields.get(&cache_key(goal))
    }

    /// Return the field for `goal`, building and caching it on a miss.
    ///
    /// Returns `None` when the goal is not finite or the graph has no node for
    /// it; the miss is still counted. When the cache is full, the oldest
    /// inserted field is evicted first.
    pub fn get_or_build(&mut self, graph: &RoadGraph, goal: [f32; 2]) -> Option<&FlowField> {
        let key = cache_key(goal);
        if self.fields.contains_key(&key) {
            self.hits += 1;
            return self.fields.get(&key);
        }
        self.misses += 1;
        let field = graph.flow_to(goal)?;
        if self.fields.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.fields.remove(&oldest);
            }
        }
        self.order.push_back(key);
        self.fields.insert(key, field);
        self.fields.get(&key)
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

    /// Lookups that built a field, including invalid goals.
    #[must_use]
    pub const fn misses(&self) -> u64 {
        self.misses
    }

    /// Drop every cached field. Hit and miss counters are cumulative for the
    /// lifetime of the cache.
    pub fn clear(&mut self) {
        self.fields.clear();
        self.order.clear();
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

        let mut edges: BTreeMap<(RoadNode, RoadNode), u32> = BTreeMap::new();
        let mut contributing_roads = 0_usize;

        for road in roads {
            let start = road.start;
            let end = road.end;
            let horizontal = start[1] == end[1];
            let vertical = start[0] == end[0];
            if horizontal == vertical {
                continue;
            }
            contributing_roads += 1;

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
            for pair in points.windows(2) {
                let a = pair[0];
                let b = pair[1];
                if a == b {
                    continue;
                }
                let length = u32::try_from(
                    (i64::from(b[0]) - i64::from(a[0])).unsigned_abs()
                        + (i64::from(b[1]) - i64::from(a[1])).unsigned_abs(),
                )
                .unwrap_or(u32::MAX);
                let key = if a <= b { (a, b) } else { (b, a) };
                edges
                    .entry(key)
                    .and_modify(|existing| *existing = (*existing).min(length))
                    .or_insert(length);
            }
        }

        let mut adjacency: BTreeMap<RoadNode, Vec<(RoadNode, u32)>> = BTreeMap::new();
        for (&(a, b), &length) in &edges {
            adjacency.entry(a).or_default().push((b, length));
            adjacency.entry(b).or_default().push((a, length));
        }
        if adjacency.len() > MAX_TRAFFIC_NODES {
            return Err(TrafficError::TooManyNodes(adjacency.len()));
        }
        for neighbors in adjacency.values_mut() {
            neighbors.sort_unstable_by_key(|left| left.0);
        }
        let nodes: Vec<RoadNode> = adjacency.keys().copied().collect();
        Ok(Self {
            adjacency,
            nodes,
            road_count: contributing_roads,
        })
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

    fn nearest_node(&self, point: [f32; 2]) -> Option<(RoadNode, f32)> {
        let mut best: Option<(RoadNode, f32)> = None;
        for &node in &self.nodes {
            let dx = node[0] as f32 - point[0];
            let dz = node[1] as f32 - point[1];
            let distance = (dx * dx + dz * dz).sqrt();
            let replace = best.is_none_or(|(best_node, best_distance)| {
                distance < best_distance || (distance == best_distance && node < best_node)
            });
            if replace {
                best = Some((node, distance));
            }
        }
        best
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
        assert!(cache.get(goal).is_some());

        cache.clear();
        assert!(cache.is_empty());
        assert!(cache.get(goal).is_none());
        assert_eq!(cache.hits(), 1);
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
}
