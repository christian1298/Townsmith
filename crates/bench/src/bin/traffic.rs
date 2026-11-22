//! Road-graph build, pair-query, flow-field reuse, and congestion microbenchmark.
#![recursion_limit = "256"]
//!
//! Builds a deterministic square road grid, then measures graph construction,
//! a fixed batch of point-to-point commute queries, one flow field with batched
//! queries, repeated-goal flow-field reuse through `FlowFieldCache`, and an
//! aggregated commute-flow congestion report. It is derived state: no save
//! file, renderer, or game loop is involved.

use std::time::Instant;

use core_city::{
    CommuteFlow, FlowField, FlowFieldCache, RoadGraph, RoadSegment, MAX_CONGESTION_FLOWS,
};

const DEFAULT_GRID: usize = 64;
const DEFAULT_QUERIES: usize = 1_000;
const DEFAULT_GOALS: usize = 64;
const DEFAULT_CONGESTION_FLOWS: usize = 1_000;
const CONGESTION_HUBS: usize = 16;
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

    // Incremental append: add 64 crossing roads one at a time and compare the
    // result with a fresh build over the combined segment list.
    let append_count = 64.min(grid);
    let mut extra = Vec::with_capacity(append_count);
    for index in 0..append_count {
        extra.push(RoadSegment {
            id: roads.len() as u64 + index as u64 + 1,
            start: [0, index as i32 * SPACING_M + 8],
            end: [extent, index as i32 * SPACING_M + 8],
            width_m: 8,
        });
    }
    let mut appended = RoadGraph::build(&roads).expect("traffic graph append base");
    let append_start = Instant::now();
    for road in &extra {
        assert!(
            appended.add_segment(road).expect("append road"),
            "new road must add graph structure"
        );
    }
    let append_ms = append_start.elapsed().as_secs_f64() * 1_000.0;
    let mut combined = roads.clone();
    combined.extend(extra.iter().cloned());
    let append_rebuild_start = Instant::now();
    let rebuilt = RoadGraph::build(&combined).expect("traffic graph combined rebuild");
    let append_rebuild_ms = append_rebuild_start.elapsed().as_secs_f64() * 1_000.0;
    let mut single = RoadGraph::build(&roads).expect("traffic graph single base");
    let append_single_start = Instant::now();
    single.add_segment(&extra[0]).expect("single append road");
    let append_single_ms = append_single_start.elapsed().as_secs_f64() * 1_000.0;
    assert_eq!(
        appended.checksum(),
        rebuilt.checksum(),
        "incremental append changed the graph"
    );

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

    // Graph edits: a field cached before an append must not be reused after
    // it. Build a deliberately disconnected fixture, cache the goal field,
    // append the connecting road, and compare the rebuilt cache entry with a
    // fresh `flow_to` on the edited graph.
    let mut edit_graph = RoadGraph::build(&[
        RoadSegment {
            id: 1,
            start: [0, 0],
            end: [64, 0],
            width_m: 8,
        },
        RoadSegment {
            id: 2,
            start: [64, 0],
            end: [64, 64],
            width_m: 8,
        },
        RoadSegment {
            id: 3,
            start: [128, 0],
            end: [192, 0],
            width_m: 8,
        },
    ])
    .expect("edit graph");
    let edit_goal = [64.0, 64.0];
    let mut edit_cache = FlowFieldCache::new(4);
    let base_edit_checksum = edit_cache
        .get_or_build(&edit_graph, edit_goal)
        .expect("base edit field")
        .checksum();
    let base_edit_nodes = edit_cache
        .get(edit_goal)
        .expect("base edit field")
        .reachable_nodes();
    let base_version = edit_graph.change_version();
    assert!(edit_graph
        .add_segment(&RoadSegment {
            id: 4,
            start: [64, 0],
            end: [128, 0],
            width_m: 8,
        })
        .expect("append edit road"));
    let edited_version = edit_graph.change_version();
    assert_ne!(
        edited_version, base_version,
        "append must move the graph version"
    );

    let edit_rebuild_start = Instant::now();
    let rebuilt_checksum = edit_cache
        .get_or_build(&edit_graph, edit_goal)
        .expect("rebuilt edit field")
        .checksum();
    let edit_rebuild_ms = edit_rebuild_start.elapsed().as_secs_f64() * 1_000.0;
    let edit_rebuilds = edit_cache.rebuilds();
    let edited_nodes = edit_cache
        .get(edit_goal)
        .expect("rebuilt edit field")
        .reachable_nodes();
    let edit_fresh_start = Instant::now();
    let edit_fresh_checksum = edit_graph
        .flow_to(edit_goal)
        .expect("fresh edit field")
        .checksum();
    let edit_fresh_ms = edit_fresh_start.elapsed().as_secs_f64() * 1_000.0;
    assert_eq!(
        rebuilt_checksum, edit_fresh_checksum,
        "cache returned a field from the pre-edit graph"
    );
    assert_ne!(
        rebuilt_checksum, base_edit_checksum,
        "the appended road did not change the field"
    );
    assert_eq!(edit_rebuilds, 1, "stale entry must rebuild once");

    let edit_hit_start = Instant::now();
    assert_eq!(
        edit_cache
            .get_or_build(&edit_graph, edit_goal)
            .expect("cached edit field")
            .checksum(),
        edit_fresh_checksum
    );
    let edit_hit_ms = edit_hit_start.elapsed().as_secs_f64() * 1_000.0;
    assert_eq!(edit_cache.hits(), 1);
    assert_eq!(edit_cache.rebuilds(), 1);

    // Congestion v0: aggregate 1,000 synthetic commute flows into undirected
    // edge loads. Origins are deterministic random nodes; destinations come
    // from a small hub set so routes share corridors. The report is diagnostic
    // and does not feed assignment or stored travel time.
    let congestion_flows = commute_flows(DEFAULT_CONGESTION_FLOWS, extent);
    let congestion_start = Instant::now();
    let congestion = graph
        .congestion(&congestion_flows, MAX_CONGESTION_FLOWS)
        .expect("congestion report");
    let congestion_ms = congestion_start.elapsed().as_secs_f64() * 1_000.0;
    let congestion_repeat = graph
        .congestion(&congestion_flows, MAX_CONGESTION_FLOWS)
        .expect("repeat congestion report");
    assert_eq!(
        congestion.checksum(),
        congestion_repeat.checksum(),
        "congestion report is not deterministic"
    );
    assert_eq!(congestion.top_edges, congestion_repeat.top_edges);
    assert_eq!(congestion.routed_flows, congestion_flows.len());

    let peak_edge_json = congestion.peak_edge.map(|edge| {
        serde_json::json!({
            "from": edge.from,
            "to": edge.to,
            "length_m": edge.length_m,
            "load": edge.load,
            "capacity": edge.capacity,
            "factor": edge.factor,
        })
    });
    let top_edges_json: Vec<serde_json::Value> = congestion
        .top_edges
        .iter()
        .map(|edge| {
            serde_json::json!({
                "from": edge.from,
                "to": edge.to,
                "length_m": edge.length_m,
                "load": edge.load,
                "factor": edge.factor,
            })
        })
        .collect();

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
        "flow_cache_graph_edits": {
            "goal": edit_goal,
            "base_reachable_nodes": base_edit_nodes,
            "edited_reachable_nodes": edited_nodes,
            "version_changed": edited_version != base_version,
            "base_checksum": format!("{base_edit_checksum:016x}"),
            "edited_checksum": format!("{rebuilt_checksum:016x}"),
            "fresh_checksum": format!("{edit_fresh_checksum:016x}"),
            "matches_fresh_build": rebuilt_checksum == edit_fresh_checksum,
            "rebuilds": edit_rebuilds,
            "rebuild_ms": edit_rebuild_ms,
            "fresh_ms": edit_fresh_ms,
            "repeat_hit_ms": edit_hit_ms,
            "hits": edit_cache.hits(),
            "misses": edit_cache.misses(),
        },
        "append_roads": append_count,
        "append_ms": append_ms,
        "append_rebuild_ms": append_rebuild_ms,
        "append_speedup": if append_ms > 0.0 { append_rebuild_ms / append_ms } else { 0.0 },
        "append_checksum": format!("{:016x}", appended.checksum()),
        "append_rebuild_checksum": format!("{:016x}", rebuilt.checksum()),
        "append_single_ms": append_single_ms,
        "append_single_speedup": if append_single_ms > 0.0 { append_rebuild_ms / append_single_ms } else { 0.0 },
        "congestion": {
            "flows": congestion_flows.len(),
            "routed_flows": congestion.routed_flows,
            "skipped_flows": congestion.skipped_flows,
            "trips": congestion.total_trips,
            "route_ms": congestion_ms,
            "edges": congestion.edges,
            "overloaded_edges": congestion.overloaded_edges,
            "average_factor": congestion.average_factor,
            "peak_factor": congestion.peak_factor,
            "peak_load": congestion.peak_load,
            "peak_edge": peak_edge_json,
            "total_route_length_m": congestion.total_route_length_m,
            "top_edges": top_edges_json,
            "checksum": format!("{:016x}", congestion.checksum()),
        },
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

/// Deterministic commute flows: scattered origins, a small hub set as
/// destinations, and 1–3 trips per flow so corridors overload.
fn commute_flows(count: usize, extent: i32) -> Vec<CommuteFlow> {
    let hubs = goal_points(CONGESTION_HUBS, extent);
    let mut seed = 0x00C0_FFEE_1234_5678_u64;
    (0..count)
        .map(|index| CommuteFlow {
            from: [next_coord(&mut seed, extent), next_coord(&mut seed, extent)],
            to: hubs[index % hubs.len()],
            trips: 1 + (index % 3) as u32,
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

    #[test]
    fn a_cached_flow_field_follows_an_appended_road() {
        let roads = [
            RoadSegment {
                id: 1,
                start: [0, 0],
                end: [64, 0],
                width_m: 8,
            },
            RoadSegment {
                id: 2,
                start: [64, 0],
                end: [64, 64],
                width_m: 8,
            },
            RoadSegment {
                id: 3,
                start: [128, 0],
                end: [192, 0],
                width_m: 8,
            },
        ];
        let connector = RoadSegment {
            id: 4,
            start: [64, 0],
            end: [128, 0],
            width_m: 8,
        };
        let goal = [64.0, 64.0];
        let mut graph = RoadGraph::build(&roads).expect("graph");
        let mut cache = FlowFieldCache::new(2);
        let base = cache
            .get_or_build(&graph, goal)
            .expect("base field")
            .checksum();

        assert!(graph.add_segment(&connector).expect("append"));
        let cached = cache
            .get_or_build(&graph, goal)
            .expect("rebuilt field")
            .checksum();

        let mut combined = roads.to_vec();
        combined.push(connector);
        let fresh = RoadGraph::build(&combined).expect("fresh graph");
        assert_eq!(graph.checksum(), fresh.checksum());
        assert_eq!(cached, fresh.flow_to(goal).expect("fresh field").checksum());
        assert_ne!(cached, base, "the append must change the field");
        assert_eq!(cache.rebuilds(), 1);
        assert_eq!(cache.hits(), 0);
        assert_eq!(cache.misses(), 2);
    }
}
