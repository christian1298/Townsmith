//! Phase 0 2,000 active-agent tick microbenchmark.

use std::hint::black_box;
use std::time::Instant;

use core_sim::{ActiveAgentSim, DEFAULT_AGENT_COUNT, TICK_DT};

const BUDGET_MS: f64 = 10.0;

fn percentile(samples: &[f64], fraction: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let index = ((samples.len() - 1) as f64 * fraction).round() as usize;
    samples[index]
}

fn main() {
    let measured_ticks = 100usize;
    let warmup_ticks = 20usize;

    let mut sim = ActiveAgentSim::new(DEFAULT_AGENT_COUNT, 0x5EED);
    for _ in 0..warmup_ticks {
        sim.tick();
    }

    let mut samples = Vec::with_capacity(measured_ticks);
    let total_start = Instant::now();
    for _ in 0..measured_ticks {
        let start = Instant::now();
        sim.tick();
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let total_ms = total_start.elapsed().as_secs_f64() * 1000.0;
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let median = percentile(&samples, 0.50);
    let p95 = percentile(&samples, 0.95);
    let max = samples.last().copied().unwrap_or_default();
    let tick_rate_hz = measured_ticks as f64 / (total_ms / 1000.0);
    let budget_use_pct = median / BUDGET_MS * 100.0;
    let agent_count = sim.agent_count();
    let checksum = black_box(
        sim.agents()
            .iter()
            .map(|agent| agent.position[0] + agent.position[1])
            .sum::<f32>(),
    );

    println!(
        concat!(
            "{{\"scene\":\"active_agents_2k\",\"agents\":{},\"ticks\":{},",
            "\"tick_dt\":{:.1},\"warmup_ticks\":{},\"total_ms\":{:.4},",
            "\"tick_ms_median\":{:.4},\"tick_ms_p95\":{:.4},",
            "\"tick_ms_max\":{:.4},\"tick_rate_hz\":{:.1},",
            "\"budget_ms\":{:.1},\"budget_use_pct\":{:.3},",
            "\"position_checksum\":{:.4}}}"
        ),
        agent_count,
        measured_ticks,
        TICK_DT,
        warmup_ticks,
        total_ms,
        median,
        p95,
        max,
        tick_rate_hz,
        BUDGET_MS,
        budget_use_pct,
        checksum
    );
}
