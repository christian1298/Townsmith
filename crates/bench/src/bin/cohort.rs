//! Phase 0 1M-cohort tick microbenchmark.

use std::hint::black_box;
use std::time::Instant;

use core_sim::{Cohort, CohortSimulation};

fn percentile(samples: &[f64], fraction: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let index = ((samples.len() - 1) as f64 * fraction).round() as usize;
    samples[index]
}

fn main() {
    let cohort_count = 10_000usize;
    let citizens_per_cohort = 100u32;
    let district_count = 128usize;
    let measured_ticks = 100usize;

    let mut sim = CohortSimulation::new(district_count);
    for index in 0..cohort_count {
        sim.add_cohort(Cohort::new(
            (index % district_count) as u32,
            citizens_per_cohort,
            0.8,
        ))
        .expect("district exists");
    }

    for _ in 0..10 {
        sim.tick(1.0);
    }

    let mut samples = Vec::with_capacity(measured_ticks);
    for _ in 0..measured_ticks {
        let start = Instant::now();
        sim.tick(1.0);
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let median = percentile(&samples, 0.50);
    let p95 = percentile(&samples, 0.95);
    let max = samples.last().copied().unwrap_or_default();
    let population = sim.total_population();
    let happiness = black_box(sim.average_happiness());
    let need = black_box(sim.average_need());

    println!(
        concat!(
            "{{\"scene\":\"cohort_1m\",\"cohorts\":{},\"citizens\":{},",
            "\"districts\":{},\"ticks\":{},\"tick_ms_median\":{:.4},",
            "\"tick_ms_p95\":{:.4},\"tick_ms_max\":{:.4},",
            "\"population\":{},\"avg_happiness\":{:.4},",
            "\"avg_need\":{:.4},\"cohort_bytes\":{}}}"
        ),
        sim.cohort_count(),
        population,
        district_count,
        measured_ticks,
        median,
        p95,
        max,
        population,
        happiness,
        need,
        sim.cohort_count() * std::mem::size_of::<Cohort>()
    );
}
