//! Job-assignment index benchmark.
//!
//! Builds a deterministic 1,000-home / 1,000-workplace city, checks that one
//! indexed assignment tick matches an independent brute-force scan, and times
//! steady-state assignment ticks against that scan.

use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::Instant;

use core_city::{CityRuntime, Home, RoadGraph, Workplace, MAX_MOVE_IN_PER_SECOND};

const HOMES: usize = 1_000;
const WORKPLACES: usize = 1_000;
const CAPACITY: u32 = 2;
const SLOTS: u32 = 2;
const SPAN_M: f32 = 3_000.0;
const TIMED_TICKS: usize = 20;
const REFERENCE_RUNS: usize = 3;

fn main() {
    let homes = homes();
    let workplaces = workplaces();
    let mut runtime = CityRuntime::new(2026);
    runtime.set_move_in_per_second(MAX_MOVE_IN_PER_SECOND);
    runtime.tick_with_workplaces(3_600.0, &homes, &workplaces);
    runtime.set_move_in_per_second(0.0);

    let (reference_workers, reference_commute, reference_staffing) =
        brute_force_reference(&runtime, &workplaces);
    let matches = check_reference(
        &runtime,
        &reference_workers,
        &reference_commute,
        &reference_staffing,
    );

    let mut reference_samples = Vec::with_capacity(REFERENCE_RUNS);
    for _ in 0..REFERENCE_RUNS {
        let start = Instant::now();
        let output = brute_force_reference(&runtime, &workplaces);
        reference_samples.push(start.elapsed().as_secs_f64() * 1_000.0);
        black_box(output);
    }

    // Warm up, then measure steady-state ticks.
    for _ in 0..3 {
        runtime.tick_with_workplaces(1.0, &homes, &workplaces);
    }
    let mut samples = Vec::with_capacity(TIMED_TICKS);
    for _ in 0..TIMED_TICKS {
        let start = Instant::now();
        runtime.tick_with_workplaces(1.0, &homes, &workplaces);
        samples.push(start.elapsed().as_secs_f64() * 1_000.0);
        black_box(runtime.workers());
    }

    samples.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    reference_samples
        .sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let tick_median = samples[samples.len() / 2];
    let tick_p95 = samples[(samples.len() - 1) * 95 / 100];
    let reference_median = reference_samples[reference_samples.len() / 2];

    println!(
        concat!(
            "{{\"scene\":\"job_index\",\"homes\":{},\"workplaces\":{},",
            "\"pairs\":{},\"residents\":{},\"workers\":{},",
            "\"tick_ms_median\":{:.4},\"tick_ms_p95\":{:.4},",
            "\"reference_ms_median\":{:.4},\"speedup\":{:.2},",
            "\"matches_reference\":{}}}"
        ),
        HOMES,
        WORKPLACES,
        HOMES * WORKPLACES,
        runtime.population(),
        runtime.workers(),
        tick_median,
        tick_p95,
        reference_median,
        reference_median / tick_median.max(f64::MIN_POSITIVE),
        matches
    );

    if !matches {
        std::process::exit(1);
    }
}

fn homes() -> Vec<Home> {
    let mut seed = 0x5eed_2026_u64;
    (0..HOMES)
        .map(|index| Home {
            id: u64::try_from(index).expect("id") + 1,
            capacity: CAPACITY,
            center: [jitter(&mut seed, SPAN_M), jitter(&mut seed, SPAN_M)],
        })
        .collect()
}

fn workplaces() -> Vec<Workplace> {
    let mut seed = 0x00c0_ffee_2026_u64;
    (0..WORKPLACES)
        .map(|index| Workplace {
            id: 1_000_000 + u64::try_from(index).expect("id"),
            slots: SLOTS,
            center: [jitter(&mut seed, SPAN_M), jitter(&mut seed, SPAN_M)],
        })
        .collect()
}

fn lcg_next(seed: &mut u64) -> u32 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    u32::try_from(*seed >> 33).unwrap_or(0)
}

fn jitter(seed: &mut u64, span: f32) -> f32 {
    let unit = f32::from(u16::try_from(lcg_next(seed) % 65_536).unwrap_or(0)) / 65_535.0;
    unit * span
}

/// The pre-index global scan, kept as an independent reference.
#[allow(clippy::type_complexity)]
fn brute_force_reference(
    runtime: &CityRuntime,
    workplaces: &[Workplace],
) -> (BTreeMap<u64, u32>, BTreeMap<u64, f32>, BTreeMap<u64, u32>) {
    let mut pairs: Vec<(f32, u64, usize)> =
        Vec::with_capacity(runtime.households.len() * workplaces.len());
    for (&id, household) in &runtime.households {
        if household.residents == 0 {
            continue;
        }
        for (index, work) in workplaces.iter().enumerate() {
            if work.slots == 0 {
                continue;
            }
            pairs.push((
                RoadGraph::straight_line_m(household.home, work.center),
                id,
                index,
            ));
        }
    }
    pairs.sort_by(|left, right| {
        left.0
            .total_cmp(&right.0)
            .then(left.1.cmp(&right.1))
            .then(left.2.cmp(&right.2))
    });
    let mut remaining: Vec<u32> = workplaces.iter().map(|work| work.slots).collect();
    let mut need: BTreeMap<u64, u32> = runtime
        .households
        .iter()
        .filter(|(_, household)| household.residents > 0)
        .map(|(&id, household)| (id, household.residents))
        .collect();
    let mut workers: BTreeMap<u64, u32> = BTreeMap::new();
    let mut weighted: BTreeMap<u64, f32> = BTreeMap::new();
    let mut staffing: BTreeMap<u64, u32> = BTreeMap::new();
    for (distance, id, index) in pairs {
        if remaining[index] == 0 {
            continue;
        }
        let Some(left) = need.get_mut(&id) else {
            continue;
        };
        if *left == 0 {
            continue;
        }
        let take = (*left).min(remaining[index]);
        remaining[index] -= take;
        *left -= take;
        *workers.entry(id).or_insert(0) += take;
        *weighted.entry(id).or_insert(0.0) += distance * take as f32;
        let staffed = staffing.entry(workplaces[index].id).or_insert(0);
        *staffed = staffed.saturating_add(take);
    }
    (workers, weighted, staffing)
}

fn check_reference(
    runtime: &CityRuntime,
    workers: &BTreeMap<u64, u32>,
    weighted: &BTreeMap<u64, f32>,
    staffing: &BTreeMap<u64, u32>,
) -> bool {
    if runtime.staffing != *staffing {
        eprintln!("staffing map diverged from the brute-force scan");
        return false;
    }
    for (&id, household) in &runtime.households {
        let expected_workers = workers.get(&id).copied().unwrap_or(0);
        if household.workers != expected_workers {
            eprintln!(
                "household {id}: workers {} != {expected_workers}",
                household.workers
            );
            return false;
        }
        let expected_commute = if expected_workers > 0 {
            weighted[&id] / expected_workers as f32
        } else {
            0.0
        };
        if household.commute_m.to_bits() != expected_commute.to_bits() {
            eprintln!(
                "household {id}: commute {} != {expected_commute}",
                household.commute_m
            );
            return false;
        }
    }
    true
}
