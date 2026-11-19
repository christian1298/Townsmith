//! Headless Townsmith server tick harness.
//!
//! Phase 0 only: builds a cohort-scale city, runs a fixed number of ticks on
//! the CPU, and emits JSON. It does not open a socket or render anything yet.

use std::time::Instant;

use core_sim::{Cohort, CohortSimulation};

fn parse_arg(name: &str, default: usize) -> usize {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn percentile(samples: &[f64], fraction: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let index = ((samples.len() - 1) as f64 * fraction).round() as usize;
    samples[index]
}

fn main() {
    let cohort_count = parse_arg("--cohorts", 10_000).max(1);
    let district_count = parse_arg("--districts", 128).max(1);
    let ticks = parse_arg("--ticks", 100).max(1);
    let citizens_per_cohort = 100u32;

    let mut sim = CohortSimulation::new(district_count);
    for index in 0..cohort_count {
        sim.add_cohort(Cohort::new(
            (index % district_count) as u32,
            citizens_per_cohort,
            0.8,
        ))
        .expect("district exists");
    }

    let mut samples = Vec::with_capacity(ticks);
    let start = Instant::now();
    for _ in 0..ticks {
        let tick_start = Instant::now();
        sim.tick(1.0);
        samples.push(tick_start.elapsed().as_secs_f64() * 1000.0);
    }
    let total_ms = start.elapsed().as_secs_f64() * 1000.0;
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    println!(
        concat!(
            "{{\"server\":\"townsmith-server\",\"mode\":\"headless\",",
            "\"cohorts\":{},\"districts\":{},\"ticks\":{},",
            "\"citizens\":{},\"tick_ms_median\":{:.4},",
            "\"tick_ms_p95\":{:.4},\"total_ms\":{:.3},",
            "\"avg_happiness\":{:.4},\"avg_need\":{:.4}}}"
        ),
        sim.cohort_count(),
        district_count,
        ticks,
        sim.total_population(),
        percentile(&samples, 0.50),
        percentile(&samples, 0.95),
        total_ms,
        sim.average_happiness(),
        sim.average_need()
    );
}
