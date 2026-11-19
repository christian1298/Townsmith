//! Phase 0 2,000 active-agent tick microbenchmark.
//!
//! Runs a sparse default world (128 m) and a dense world (32 m) so the
//! benchmark exercises both the normal case and the neighbor cap.

use std::hint::black_box;
use std::time::Instant;

use core_sim::{ActiveAgentSim, NeighborStats, DEFAULT_AGENT_COUNT, TICK_DT};

const BUDGET_MS: f64 = 10.0;
const DENSE_WORLD_SIZE: f32 = 32.0;
const WARMUP_TICKS: usize = 20;
const MEASURED_TICKS: usize = 100;

struct Measurement {
    total_ms: f64,
    median_ms: f64,
    p95_ms: f64,
    max_ms: f64,
    checksum: f32,
    neighbors: NeighborStats,
}

fn percentile(samples: &[f64], fraction: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let index = ((samples.len() - 1) as f64 * fraction).round() as usize;
    samples[index]
}

fn measure(mut sim: ActiveAgentSim) -> Measurement {
    for _ in 0..WARMUP_TICKS {
        sim.tick();
    }

    let mut samples = Vec::with_capacity(MEASURED_TICKS);
    let total_start = Instant::now();
    for _ in 0..MEASURED_TICKS {
        let start = Instant::now();
        sim.tick();
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let total_ms = total_start.elapsed().as_secs_f64() * 1000.0;
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    Measurement {
        total_ms,
        median_ms: percentile(&samples, 0.50),
        p95_ms: percentile(&samples, 0.95),
        max_ms: samples.last().copied().unwrap_or_default(),
        checksum: black_box(
            sim.agents()
                .iter()
                .map(|agent| agent.position[0] + agent.position[1])
                .sum::<f32>(),
        ),
        neighbors: sim.neighbor_stats(),
    }
}

fn rate_hz(total_ms: f64) -> f64 {
    MEASURED_TICKS as f64 / (total_ms / 1000.0)
}

fn budget_use_pct(median_ms: f64) -> f64 {
    median_ms / BUDGET_MS * 100.0
}

fn main() {
    let sparse = measure(ActiveAgentSim::new(DEFAULT_AGENT_COUNT, 0x5EED));
    let dense = measure(ActiveAgentSim::with_world_size(
        DEFAULT_AGENT_COUNT,
        0x5EED,
        DENSE_WORLD_SIZE,
    ));

    println!(
        concat!(
            "{{\"scene\":\"active_agents_2k\",\"agents\":{},\"ticks\":{},",
            "\"tick_dt\":{:.1},\"warmup_ticks\":{},",
            "\"total_ms\":{:.4},\"tick_ms_median\":{:.4},",
            "\"tick_ms_p95\":{:.4},\"tick_ms_max\":{:.4},",
            "\"tick_rate_hz\":{:.1},\"budget_ms\":{:.1},",
            "\"budget_use_pct\":{:.3},\"position_checksum\":{:.4},",
            "\"neighbors_mean\":{:.3},\"neighbors_max\":{},\"agents_at_cap\":{},",
            "\"dense_world_size\":{:.1},\"dense_total_ms\":{:.4},",
            "\"dense_tick_ms_median\":{:.4},\"dense_tick_ms_p95\":{:.4},",
            "\"dense_tick_ms_max\":{:.4},\"dense_tick_rate_hz\":{:.1},",
            "\"dense_budget_use_pct\":{:.3},\"dense_position_checksum\":{:.4},",
            "\"dense_neighbors_mean\":{:.3},\"dense_neighbors_max\":{},",
            "\"dense_agents_at_cap\":{}}}"
        ),
        DEFAULT_AGENT_COUNT,
        MEASURED_TICKS,
        TICK_DT,
        WARMUP_TICKS,
        sparse.total_ms,
        sparse.median_ms,
        sparse.p95_ms,
        sparse.max_ms,
        rate_hz(sparse.total_ms),
        BUDGET_MS,
        budget_use_pct(sparse.median_ms),
        sparse.checksum,
        sparse.neighbors.mean,
        sparse.neighbors.max,
        sparse.neighbors.at_cap,
        DENSE_WORLD_SIZE,
        dense.total_ms,
        dense.median_ms,
        dense.p95_ms,
        dense.max_ms,
        rate_hz(dense.total_ms),
        budget_use_pct(dense.median_ms),
        dense.checksum,
        dense.neighbors.mean,
        dense.neighbors.max,
        dense.neighbors.at_cap
    );
}
