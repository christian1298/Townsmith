//! Road-graph build, pair-query, and flow-field reuse microbenchmark.
//!
//! Builds a deterministic square road grid, then measures graph construction,
//! a fixed batch of point-to-point commute queries, one flow field with batched
//! queries, and repeated-goal flow-field reuse through `FlowFieldCache`. It is
//! derived state: no save file, renderer, or game loop is involved.

use std::time::Instant;

use core_city::{FlowField, FlowFieldCache, RoadGraph, RoadSegment};

const DEFAULT_GRID: usize = 64;
const DEFAULT_QUERIES: usize = 1_000;
const DEFAULT_GOALS: usize = 64;
const CACHE_PASSES: usize = 4;
const SPACING_M: i32 = 16;

fn main() {
    let grid = arg_value("--grid")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_GRID);
    let queries = arg_value("--queries")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_QUERIES);
    let goal_count = arg_value("--goals")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_GOALS);
    if grid < 2 || queries == 0 || goal_count == 0 {
        eprintln!("usage: traffic [--grid N] [--queries N] [--goals N]");
        std::process::exit(2);
    }

    let roads = grid_roads(grid);
    let extent = (grid as i32 - 1) * SPACING_M;
    let build_start = Instant::now();
    let graph = RoadGraph::build(&roads).expect("traffic graph");
    let build_ms = build_start.elapsed().as_secs_f64() * 1_000.0;
    let stats = graph.stats();

    let mut seed = 0x2545_f491_4f6c_dd1d_u64;
    let query_start = Instant::now();
    let mut reached = 0_usize;
    let mut total_m = 0_f64;
    for _ in 0..queries {
        let from = [next_coord(&mut seed, extent), next_coord(&mut seed, extent)];
        let to = [next_coord(&mut seed, extent), next_coord(&mut seed, extent)];
        if let Some(distance) = graph.distance_m(from, to) {
            reached += 1;
            total_m += f64::from(distance);
        }
    }
    let query_ms = query_start.elapsed().as_secs_f64() * 1_000.0;

    // Nearest-node lookup: indexed grid vs the exact linear reference. The
    // points mix exact nodes, cell centres, and half-cell offsets.
    let mut near_seed = 0xa076_1d64_78bd_642f_u64;
    let near_points: Vec<[f32; 2]> = (0..queries)
        .map(|index| {
            if index % 4 == 0 {
                let node = (next_coord(&mut near_seed, extent) / SPACING_M as f32).round()
                    * SPACING_M as f32;
                [node, node]
            } else {
                [
                    next_coord(&mut near_seed, extent) + 0.5,
                    next_coord(&mut near_seed, extent) + 0.5,
                ]
            }
        })
        .collect();
    // Warm the grid path before timing.
    let _ = graph.nearest_node(near_points[0]);
    let near_index_start = Instant::now();
    let indexed: Vec<Option<(core_city::RoadNode, f32)>> = near_points
        .iter()
        .map(|&point| graph.nearest_node(point))
        .collect();
    let near_index_ms = near_index_start.elapsed().as_secs_f64() * 1_000.0;
    let near_linear_start = Instant::now();
    let linear: Vec<Option<(core_city::RoadNode, f32)>> = near_points
        .iter()
        .map(|&point| graph.nearest_node_linear(point))
        .collect();
    let near_linear_ms = near_linear_start.elapsed().as_secs_f64() * 1_000.0;
    assert_eq!(
        indexed, linear,
        "nearest-node index changed a lookup result"
    );
    let mut near_hash = 0xcbf2_9ce4_8422_2325_u64;
    for (node, distance) in indexed.iter().flatten() {
        near_hash ^= (node[0] as i64 as u64) << 32 ^ (node[1] as i64 as u64);
        near_hash ^= u64::from(distance.to_bits());
        near_hash = near_hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let near_checksum = near_hash;

    let flow_start = Instant::now();
    let flow = graph
        .flow_to([extent as f32, extent as f32])
        .expect("flow field");
    let flow_build_ms = flow_start.elapsed().as_secs_f64() * 1_000.0;
    let mut flow_seed = 0x9e37_79b9_7f4a_7c15_u64;
    let flow_start = Instant::now();
    let mut flow_reached = 0_usize;
    let mut flow_total_m = 0_f64;
    for _ in 0..queries {
        let from = [
            next_coord(&mut flow_seed, extent),
            next_coord(&mut flow_seed, extent),
        ];
        if let Some(distance) = graph.flow_distance_m(from, &flow) {
            flow_reached += 1;
            flow_total_m += f64::from(distance);
        }
    }
    let flow_query_ms = flow_start.elapsed().as_secs_f64() * 1_000.0;

    // Distinct goal fields, built without reuse: the cost a caller pays when
    // every destination is new.
    let goals = goal_points(goal_count, extent);
    let goal_build_start = Instant::now();
    let uncached: Vec<FlowField> = goals
        .iter()
        .filter_map(|&goal| graph.flow_to(goal))
        .collect();
    let goal_flow_build_ms = goal_build_start.elapsed().as_secs_f64() * 1_000.0;
    let uncached_checksum = fields_checksum(uncached.iter().map(FlowField::checksum));

    // The same goals through the bounded cache. Pass one builds each field;
    // later passes hit and reuse it.
    let mut cache = FlowFieldCache::new(goals.len());
    let cache_start = Instant::now();
    let mut cached_nodes = 0_usize;
    for _ in 0..CACHE_PASSES {
        for &goal in &goals {
            let field = cache.get_or_build(&graph, goal).expect("cached flow field");
            cached_nodes += field.reachable_nodes();
        }
    }
    let goal_flow_cache_ms = cache_start.elapsed().as_secs_f64() * 1_000.0;
    let cached_checksum = fields_checksum(
        goals
            .iter()
            .map(|&goal| cache.get(goal).expect("cached field").checksum()),
    );
    assert_eq!(
        uncached_checksum, cached_checksum,
        "flow cache changed field contents"
    );

    // Query the cached fields from fixed origins. These are all cache hits.
    let mut goal_query_seed = 0xd1b5_4a32_d192_ed03_u64;
    let goal_query_start = Instant::now();
    let mut goal_reached = 0_usize;
    let mut goal_total_m = 0_f64;
    for index in 0..queries {
        let goal = goals[index % goals.len()];
        let from = [
            next_coord(&mut goal_query_seed, extent),
            next_coord(&mut goal_query_seed, extent),
        ];
        let field = cache.get_or_build(&graph, goal).expect("cached flow field");
        if let Some(distance) = graph.flow_distance_m(from, field) {
            goal_reached += 1;
            goal_total_m += f64::from(distance);
        }
    }
    let goal_flow_query_ms = goal_query_start.elapsed().as_secs_f64() * 1_000.0;

    let result = serde_json::json!({
        "scene": "traffic",
        "grid": grid,
        "spacing_m": SPACING_M,
        "roads": roads.len(),
        "nodes": stats.nodes,
        "edges": stats.edges,
        "components": stats.components,
        "connected": stats.connected(),
        "total_length_m": stats.total_length_m,
        "build_ms": build_ms,
        "queries": queries,
        "reached": reached,
        "query_ms": query_ms,
        "near_queries": queries,
        "near_index_ms": near_index_ms,
        "near_linear_ms": near_linear_ms,
        "near_speedup": if near_index_ms > 0.0 { near_linear_ms / near_index_ms } else { 0.0 },
        "near_checksum": format!("{near_checksum:016x}"),
        "average_query_m": if reached == 0 { 0.0 } else { total_m / reached as f64 },
        "flow_build_ms": flow_build_ms,
        "flow_reached": flow_reached,
        "flow_query_ms": flow_query_ms,
        "flow_average_m": if flow_reached == 0 { 0.0 } else { flow_total_m / flow_reached as f64 },
        "goals": goal_count,
        "goal_flow_build_ms": goal_flow_build_ms,
        "goal_flow_reachable_nodes": uncached.len(),
        "goal_flow_cache_ms": goal_flow_cache_ms,
        "goal_flow_cache_passes": CACHE_PASSES,
        "goal_flow_cache_capacity": cache.capacity(),
        "goal_flow_cache_hits": cache.hits(),
        "goal_flow_cache_misses": cache.misses(),
        "goal_flow_cache_nodes": cached_nodes,
        "goal_flow_reached": goal_reached,
        "goal_flow_query_ms": goal_flow_query_ms,
        "goal_flow_average_m": if goal_reached == 0 { 0.0 } else { goal_total_m / goal_reached as f64 },
        "flow_checksum": format!("{:016x}", cached_checksum),
        "checksum": format!("{:016x}", graph.checksum()),
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&result).expect("pretty json")
    );
}

fn grid_roads(grid: usize) -> Vec<RoadSegment> {
    let extent = (grid as i32 - 1) * SPACING_M;
    let mut roads = Vec::with_capacity(grid * 2);
    let mut id = 1_u64;
    for index in 0..grid {
        let offset = index as i32 * SPACING_M;
        roads.push(RoadSegment {
            id,
            start: [0, offset],
            end: [extent, offset],
            width_m: 8,
        });
        id += 1;
        roads.push(RoadSegment {
            id,
            start: [offset, 0],
            end: [offset, extent],
            width_m: 8,
        });
        id += 1;
    }
    roads
}

/// Distinct goal points spread deterministically over the grid extent.
fn goal_points(count: usize, extent: i32) -> Vec<[f32; 2]> {
    let span = u64::try_from(extent).unwrap_or(1) + 1;
    (0..count)
        .map(|index| {
            let index = index as u64;
            [
                ((index * 131 + 17) % span) as f32,
                ((index * 197 + 43) % span) as f32,
            ]
        })
        .collect()
}

fn fields_checksum(checksums: impl IntoIterator<Item = u64>) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for value in checksums {
        hash ^= value;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn next_coord(seed: &mut u64, extent: i32) -> f32 {
    *seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    ((*seed >> 33) % (extent as u64 + 1)) as f32
}

fn arg_value(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    for index in 0..args.len() {
        if args[index] == name {
            return args.get(index + 1).cloned();
        }
        let prefix = format!("{name}=");
        if let Some(value) = args[index].strip_prefix(&prefix) {
            return Some(value.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_square_grid_is_connected() {
        let graph = RoadGraph::build(&grid_roads(4)).expect("graph");
        let stats = graph.stats();
        assert_eq!(stats.nodes, 16);
        assert_eq!(stats.edges, 24);
        assert_eq!(stats.components, 1);
        assert!(stats.connected());
        assert!(graph.distance_m([0.0, 0.0], [48.0, 48.0]).is_some());
    }

    #[test]
    fn a_sixty_four_grid_is_connected_at_full_size() {
        let graph = RoadGraph::build(&grid_roads(64)).expect("graph");
        let stats = graph.stats();
        assert_eq!(stats.roads, 128);
        assert_eq!(stats.nodes, 4_096);
        assert_eq!(stats.edges, 8_064);
        assert_eq!(stats.components, 1);
        assert!(stats.connected());
        assert!((stats.total_length_m - 129_024.0).abs() < 0.001);
        assert!(
            (graph
                .distance_m([0.0, 0.0], [1_008.0, 1_008.0])
                .expect("path")
                - 2_016.0)
                .abs()
                < 0.001
        );
    }

    #[test]
    fn generated_goal_points_are_distinct() {
        let goals = goal_points(DEFAULT_GOALS, 1_008);
        assert_eq!(goals.len(), DEFAULT_GOALS);
        for (index, goal) in goals.iter().enumerate() {
            assert!(goal[0] >= 0.0 && goal[0] <= 1_008.0);
            assert!(goal[1] >= 0.0 && goal[1] <= 1_008.0);
            assert!(!goals[..index].contains(goal), "duplicate goal at {index}");
        }
    }
}
