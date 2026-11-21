//! Road-graph build and flow-field microbenchmark.
//!
//! Builds a square road grid, then measures graph construction and a fixed
//! batch of point-to-point commute queries. It is derived state: no save file,
//! renderer, or game loop is involved.

use std::time::Instant;

use core_city::{RoadGraph, RoadSegment};

const DEFAULT_GRID: usize = 32;
const DEFAULT_QUERIES: usize = 1_000;
const SPACING_M: i32 = 16;

fn main() {
    let grid = arg_value("--grid")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_GRID);
    let queries = arg_value("--queries")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_QUERIES);
    if grid < 2 || queries == 0 {
        eprintln!("usage: traffic [--grid N] [--queries N]");
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
        "average_query_m": if reached == 0 { 0.0 } else { total_m / reached as f64 },
        "flow_build_ms": flow_build_ms,
        "flow_reached": flow_reached,
        "flow_query_ms": flow_query_ms,
        "flow_average_m": if flow_reached == 0 { 0.0 } else { flow_total_m / flow_reached as f64 },
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
}
