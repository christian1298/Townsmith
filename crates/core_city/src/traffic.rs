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
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

use crate::road::RoadSegment;

/// Maximum road segments accepted by one traffic graph.
pub const MAX_TRAFFIC_ROADS: usize = 10_000;

/// Maximum graph nodes accepted by one traffic graph.
pub const MAX_TRAFFIC_NODES: usize = 20_000;

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
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        let mut feed = |value: u64| {
            hash ^= value;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        };
        for &node in &self.nodes {
            feed(node[0] as u32 as u64);
            feed(node[1] as u32 as u64);
            if let Some(neighbors) = self.adjacency.get(&node) {
                for &(next, length) in neighbors {
                    feed(next[0] as u32 as u64);
                    feed(next[1] as u32 as u64);
                    feed(u64::from(length));
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
    fn checksum_is_stable_and_changes_with_geometry() {
        let first = RoadGraph::build(&[road(1, [0, 0], [10, 0])]).expect("build");
        let second = RoadGraph::build(&[road(1, [0, 0], [10, 0])]).expect("build");
        let third = RoadGraph::build(&[road(1, [0, 0], [12, 0])]).expect("build");
        assert_eq!(first.checksum(), second.checksum());
        assert_ne!(first.checksum(), third.checksum());
    }
}
