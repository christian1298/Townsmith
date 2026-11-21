//! Headless 1M-population data-scale harness for the metropolis gate.
//!
//! This is a **data harness**, not a rendering benchmark. It builds 10,000
//! cohorts (1,000,000 residents), 250,000 compact building records, and 2,000
//! active agents, then runs 120 deterministic city ticks. Each 1 Hz city tick
//! also runs ten 10 Hz agent steps in each of a sparse (128 m) and a dense
//! (32 m) world. There are no wall-clock sleeps and no render data is
//! allocated.
//!
//! The result is one JSON object with timing percentiles and a state checksum.
//! `--smoke` runs the whole scene twice and fails unless the checksum (and the
//! aggregate state it covers) is identical.
//!
//! See `docs/68 - Metropolis Gate.md` §7.4. This harness does not replace the
//! fixed 1M save, the full 1 Hz pipeline, or a gate run.

use std::hint::black_box;
use std::time::Instant;

use core_sim::{ActiveAgentSim, Cohort, CohortSimulation, Pcg32, DEFAULT_WORLD_SIZE};

/// Fixed seed for the whole scene.
const SEED: u64 = 0x4D45_5452_4F50_4F4C; // "METROPOL"
const DEFAULT_COHORTS: usize = 10_000;
const CITIZENS_PER_COHORT: u32 = 100;
const DEFAULT_DISTRICTS: usize = 128;
const DEFAULT_AGENTS: usize = 2_000;
const DEFAULT_BUILDINGS: usize = 250_000;
const DEFAULT_TICKS: usize = 120;
/// Agent steps per 1 Hz city tick: the D-017 10 Hz active tier.
const AGENT_STEPS_PER_TICK: usize = 10;
/// Side of the dense agent world, in meters. The sparse profile uses
/// [`DEFAULT_WORLD_SIZE`] (128 m).
const DENSE_WORLD_SIZE: f32 = 32.0;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Scene parameters. Tests use a smaller configuration of the same scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Config {
    seed: u64,
    cohorts: usize,
    citizens_per_cohort: u32,
    districts: usize,
    agents: usize,
    buildings: usize,
    ticks: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            seed: SEED,
            cohorts: DEFAULT_COHORTS,
            citizens_per_cohort: CITIZENS_PER_COHORT,
            districts: DEFAULT_DISTRICTS,
            agents: DEFAULT_AGENTS,
            buildings: DEFAULT_BUILDINGS,
            ticks: DEFAULT_TICKS,
        }
    }
}

/// A compact building record: 16 bytes, no blueprint strings, no mesh, no
/// render data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BuildingRecord {
    district: u16,
    kind: u8,
    stories: u8,
    footprint_m2: u16,
    resident_capacity: u16,
    job_slots: u16,
    /// Condition in per-mille units, 0..=1000.
    condition_q: u16,
    built_tick: u32,
}

/// Measured state after the scene finishes.
#[derive(Debug)]
struct Outcome {
    config: Config,
    population: u64,
    employed: u64,
    cohort_count: usize,
    agent_count: usize,
    sparse_agents: usize,
    dense_agents: usize,
    building_count: usize,
    building_bytes: usize,
    average_happiness: f32,
    average_need: f32,
    checksum: u64,
    setup_ms: f64,
    tick_loop_ms: f64,
    total_ms: f64,
    tick_samples: Vec<f64>,
    cohort_samples: Vec<f64>,
    agent_samples: Vec<f64>,
}

fn main() {
    if std::env::args().any(|arg| arg == "--smoke") {
        run_smoke();
        return;
    }

    let outcome = simulate(Config::default());
    let report = serde_json::to_string(&report(&outcome)).expect("json");
    println!("{report}");
}

/// Run the default scene twice and fail if the state checksum differs.
fn run_smoke() {
    let first = simulate(Config::default());
    let second = simulate(Config::default());
    let deterministic = first.checksum == second.checksum
        && first.population == second.population
        && first.employed == second.employed
        && first.average_happiness.to_bits() == second.average_happiness.to_bits()
        && first.average_need.to_bits() == second.average_need.to_bits();
    let expected_population = (Config::default().cohorts * CITIZENS_PER_COHORT as usize) as u64;
    let ok = deterministic
        && first.population == expected_population
        && first.cohort_count == Config::default().cohorts;

    println!(
        "METROPOLIS_SMOKE population={} cohorts={} agents={} buildings={} \
         ticks={} checksum={:016x} deterministic={} status={}",
        first.population,
        first.cohort_count,
        first.agent_count,
        first.building_count,
        first.config.ticks,
        first.checksum,
        deterministic,
        if ok { "ok" } else { "failed" }
    );
    if !ok {
        std::process::exit(1);
    }
}

fn simulate(config: Config) -> Outcome {
    let scene_start = Instant::now();
    let setup_start = Instant::now();

    let buildings = build_buildings(config.seed, config.buildings, config.districts);
    let building_bytes = buildings.len() * std::mem::size_of::<BuildingRecord>();

    let mut city = CohortSimulation::new(config.districts);
    for index in 0..config.cohorts {
        city.add_cohort(Cohort::new(
            (index % config.districts) as u32,
            config.citizens_per_cohort,
            0.8,
        ))
        .expect("district exists");
    }

    // Split the active tier across the two documented profiles so the same
    // scene covers sparse separation and the dense neighbor cap.
    let sparse_agents = config.agents / 2;
    let dense_agents = config.agents - sparse_agents;
    let mut sparse = ActiveAgentSim::with_world_size(
        sparse_agents,
        config.seed ^ 0x5A17_5EED,
        DEFAULT_WORLD_SIZE,
    );
    let mut dense =
        ActiveAgentSim::with_world_size(dense_agents, config.seed ^ 0xD3A5_E5EED, DENSE_WORLD_SIZE);

    let setup_ms = ms(setup_start.elapsed());

    let mut tick_samples = Vec::with_capacity(config.ticks);
    let mut cohort_samples = Vec::with_capacity(config.ticks);
    let mut agent_samples = Vec::with_capacity(config.ticks);

    let loop_start = Instant::now();
    for _ in 0..config.ticks {
        let tick_start = Instant::now();

        // 1 Hz cohort step.
        let cohort_start = Instant::now();
        city.tick(1.0);
        cohort_samples.push(ms(cohort_start.elapsed()));

        // 10 Hz active tier: ten steps per city tick.
        let agents_start = Instant::now();
        for _ in 0..AGENT_STEPS_PER_TICK {
            sparse.tick();
            dense.tick();
        }
        agent_samples.push(ms(agents_start.elapsed()));

        tick_samples.push(ms(tick_start.elapsed()));
    }
    let tick_loop_ms = ms(loop_start.elapsed());

    // Keep the aggregate reads in the measured work.
    let average_happiness = black_box(city.average_happiness());
    let average_need = black_box(city.average_need());
    let checksum = scene_checksum(&city, &sparse, &dense, &buildings);
    let population = city.total_population();
    let employed = city.employed_population();

    Outcome {
        config,
        population,
        employed,
        cohort_count: city.cohort_count(),
        agent_count: sparse.agent_count() + dense.agent_count(),
        sparse_agents: sparse.agent_count(),
        dense_agents: dense.agent_count(),
        building_count: buildings.len(),
        building_bytes,
        average_happiness,
        average_need,
        checksum,
        setup_ms,
        tick_loop_ms,
        total_ms: ms(scene_start.elapsed()),
        tick_samples,
        cohort_samples,
        agent_samples,
    }
}

fn report(outcome: &Outcome) -> serde_json::Value {
    let config = outcome.config;
    serde_json::json!({
        "scene": "metropolis_data_1m",
        "format_version": 1,
        "render": false,
        "note": "Headless data harness only: cohorts, active agents, and static compact building records. It has no renderer, world, save/load, pathfinding, economy, or traffic and is not the 1M / 30 FPS metropolis gate (docs/68 section 2).",
        "seed": config.seed,
        "population": outcome.population,
        "cohorts": outcome.cohort_count,
        "citizens_per_cohort": config.citizens_per_cohort,
        "districts": config.districts,
        "active_agents": outcome.agent_count,
        "active_agent_profile": "split sparse 128 m + dense 32 m",
        "sparse_agents": outcome.sparse_agents,
        "dense_agents": outcome.dense_agents,
        "buildings": outcome.building_count,
        "building_record_bytes": outcome.building_bytes,
        "ticks": config.ticks,
        "city_hz": 1,
        "agent_hz": 10,
        "agent_steps_per_profile": config.ticks * AGENT_STEPS_PER_TICK,
        "elapsed_ms": outcome.total_ms,
        "setup_ms": outcome.setup_ms,
        "tick_loop_ms": outcome.tick_loop_ms,
        "tick_ms_median": percentile(&outcome.tick_samples, 0.50),
        "tick_ms_p95": percentile(&outcome.tick_samples, 0.95),
        "tick_ms_max": max_sample(&outcome.tick_samples),
        "cohort_ms_median": percentile(&outcome.cohort_samples, 0.50),
        "cohort_ms_p95": percentile(&outcome.cohort_samples, 0.95),
        "agents_ms_median": percentile(&outcome.agent_samples, 0.50),
        "agents_ms_p95": percentile(&outcome.agent_samples, 0.95),
        "average_happiness": outcome.average_happiness,
        "average_need": outcome.average_need,
        "employed": outcome.employed,
        "checksum": format!("{:016x}", outcome.checksum),
        "checksum_scope": "cohorts + active agents + building records after the final tick",
    })
}

/// Deterministically generate `count` compact building records.
///
/// Four of every five records are residential with five resident slots, so the
/// default 250,000 records cover 1,000,000 residents. No blueprint or render
/// data is stored.
fn build_buildings(seed: u64, count: usize, districts: usize) -> Vec<BuildingRecord> {
    let mut rng = Pcg32::new(seed ^ 0xB0B_5EED);
    let mut buildings = Vec::with_capacity(count);
    for index in 0..count {
        let residential = (index * 4) % 5 < 4;
        let district = if districts == 0 {
            0
        } else {
            (index % districts) as u16
        };
        let kind = if residential {
            (index % 4) as u8
        } else {
            4 + (index % 5) as u8
        };
        let stories = 1 + (rng.next_u32() % 12) as u8;
        let footprint_m2 = 48 + (rng.next_u32() % 400) as u16;
        let resident_capacity = if residential { 5 } else { 0 };
        let job_slots = if residential {
            0
        } else {
            6 + (rng.next_u32() % 10) as u16
        };
        let condition_q = 800 + (rng.next_u32() % 201) as u16;
        let built_tick = index.wrapping_mul(2_654_435_761) as u32 % 1_000;

        buildings.push(BuildingRecord {
            district,
            kind,
            stories,
            footprint_m2,
            resident_capacity,
            job_slots,
            condition_q,
            built_tick,
        });
    }
    buildings
}

fn scene_checksum(
    city: &CohortSimulation,
    sparse: &ActiveAgentSim,
    dense: &ActiveAgentSim,
    buildings: &[BuildingRecord],
) -> u64 {
    let mut state = FNV_OFFSET;

    state = fnv_u64(state, 0x434F_484F_5254); // "COHORT"
    for cohort in city.cohorts() {
        state = fnv_u64(state, u64::from(cohort.district));
        state = fnv_u64(state, u64::from(cohort.count));
        state = fnv_u64(state, u64::from(cohort.employed));
        for need in cohort.needs {
            state = fnv_u64(state, u64::from(need.to_bits()));
        }
        state = fnv_u64(state, u64::from(cohort.happiness.to_bits()));
        state = fnv_u64(state, u64::from(cohort.income.to_bits()));
    }

    state = fnv_u64(state, 0x4147_454E_5453); // "AGENTS"
    for sim in [sparse, dense] {
        state = fnv_u64(state, sim.tick_count());
        for agent in sim.agents() {
            state = fnv_u64(state, u64::from(agent.position[0].to_bits()));
            state = fnv_u64(state, u64::from(agent.position[1].to_bits()));
            state = fnv_u64(state, u64::from(agent.velocity[0].to_bits()));
            state = fnv_u64(state, u64::from(agent.velocity[1].to_bits()));
        }
    }

    state = fnv_u64(state, 0x4255_494C_4449); // "BUILDI"
    for building in buildings {
        state = fnv_u64(state, u64::from(building.district));
        state = fnv_u64(state, u64::from(building.kind));
        state = fnv_u64(state, u64::from(building.stories));
        state = fnv_u64(state, u64::from(building.footprint_m2));
        state = fnv_u64(state, u64::from(building.resident_capacity));
        state = fnv_u64(state, u64::from(building.job_slots));
        state = fnv_u64(state, u64::from(building.condition_q));
        state = fnv_u64(state, u64::from(building.built_tick));
    }

    state
}

fn fnv_u64(mut state: u64, value: u64) -> u64 {
    for byte in value.to_le_bytes() {
        state ^= u64::from(byte);
        state = state.wrapping_mul(FNV_PRIME);
    }
    state
}

fn percentile(samples: &[f64], fraction: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted[index]
}

fn max_sample(samples: &[f64]) -> f64 {
    samples.iter().copied().fold(0.0_f64, f64::max)
}

fn ms(duration: std::time::Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_config() -> Config {
        Config {
            cohorts: 64,
            agents: 16,
            buildings: 1_000,
            ticks: 4,
            ..Config::default()
        }
    }

    #[test]
    fn default_counts_match_the_metropolis_fixture() {
        let config = Config::default();
        assert_eq!(config.cohorts, 10_000);
        assert_eq!(
            config.cohorts * config.citizens_per_cohort as usize,
            1_000_000
        );
        assert_eq!(config.agents, 2_000);
        assert_eq!(config.buildings, 250_000);
        assert_eq!(config.ticks, 120);
    }

    #[test]
    fn scene_is_deterministic_across_runs() {
        let first = simulate(small_config());
        let second = simulate(small_config());
        assert_eq!(first.checksum, second.checksum);
        assert_eq!(first.population, second.population);
        assert_eq!(first.employed, second.employed);
        assert_eq!(
            first.average_happiness.to_bits(),
            second.average_happiness.to_bits()
        );
        assert_eq!(first.population, 6_400);
        assert_eq!(first.cohort_count, 64);
        assert_eq!(first.agent_count, 16);
        assert_eq!(first.building_count, 1_000);
        assert_eq!(first.tick_samples.len(), 4);
    }

    #[test]
    fn different_seeds_produce_different_agent_state() {
        let first = simulate(small_config());
        let second = simulate(Config {
            seed: SEED ^ 0xFFFF,
            ..small_config()
        });
        assert_ne!(first.checksum, second.checksum);
    }

    #[test]
    fn building_records_are_compact_and_cover_the_population() {
        let buildings = build_buildings(SEED, 250_000, 128);
        assert_eq!(buildings.len(), 250_000);
        assert_eq!(std::mem::size_of::<BuildingRecord>(), 16);
        let capacity: u64 = buildings
            .iter()
            .map(|building| u64::from(building.resident_capacity))
            .sum();
        assert_eq!(capacity, 1_000_000);
        let jobs: u64 = buildings
            .iter()
            .map(|building| u64::from(building.job_slots))
            .sum();
        assert!(jobs > 0);
    }
}
