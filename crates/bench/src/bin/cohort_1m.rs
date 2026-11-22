//! Deterministic 1M headless cohort gate harness for docs/68 `cohort_1m_v1`.
//!
//! This harness reuses [`CohortSimulation`] and the 10 Hz [`ActiveAgentSim`]
//! tier to produce §8.2 #10–#12 evidence for the headless 1M scene:
//!
//! - 10,000 cohorts × 100 citizens = **1,000,000 population** across 128
//!   districts, each with a fixed, seed-derived service profile;
//! - 2,000 active agents split 1,000 sparse (128 m) and 1,000 dense (32 m),
//!   with ten 10 Hz agent steps inside every 1 Hz city tick;
//! - 10 discarded warm-up ticks, then **3,600 measured ticks at 1 Hz** with no
//!   sleeps, no wall-clock gates, and no render work;
//! - tick percentiles, total simulation CPU per wall-clock second, peak RSS
//!   from `/proc/self/status`, a deterministic FNV checksum, and a cohort-state
//!   encode/decode round trip.
//!
//! The output is one JSON object in the docs/68 §6.3 / §8.4 gate schema:
//! `scene`, `route`, `settings`, `duration_s`, `warmup_s`, `runs`, `headless`,
//! `hardware`, and `results`. Fields the harness cannot measure are **omitted**,
//! not faked:
//!
//! - `save_ms` / `load_ms` / `save_bytes` — there is no full world save here;
//!   only a compact cohort-state round trip is measured
//!   (`cohort_state_*`). The gate evaluator therefore reports `insufficient`.
//! - `pathfinding_ms_p99` — this scene has no pathfinding.
//! - `traffic_*` — no traffic graph.
//! - `network_stall_ms_max` — no server socket.
//! - `full_tick_ms_p99` is the measured whole 1 Hz tick. This harness has no
//!   district/building/economy aggregate, so its tick is cohort + agent work;
//!   it is a conservative superset of the cohort step, not a substitute for the
//!   full pipeline.
//!
//! `--smoke` runs 1,000 cohorts (100,000 citizens), 60 ticks, twice, and fails
//! unless the checksum and population are identical. `--ticks N`, `--cohorts N`,
//! and `--runs N` override the scene size. The default is three runs so the
//! docs/68 §6.2 three-run protocol is the default output; `--runs 1` is the
//! quick local pass.
//!
//! See `docs/68 - Metropolis Gate.md` §7.11.

#![recursion_limit = "256"]

use std::collections::BTreeSet;
use std::hint::black_box;
use std::time::Instant;

use core_sim::{
    ActiveAgentSim, Cohort, CohortSimulation, DistrictServices, Pcg32, DEFAULT_WORLD_SIZE,
};

/// Fixed seed for the whole scene.
const SEED: u64 = 0x0043_4F48_4F52_5431; // "COHORT1"
const DEFAULT_COHORTS: usize = 10_000;
const CITIZENS_PER_COHORT: u32 = 100;
const DEFAULT_DISTRICTS: usize = 128;
const DEFAULT_AGENTS: usize = 2_000;
const DEFAULT_TICKS: usize = 3_600;
/// Ticks run before measurement and discarded. At 1 Hz this is 10 simulated
/// seconds, so `warmup_s` in the report is 10.
const WARMUP_TICKS: usize = 10;
/// Agent steps per 1 Hz city tick: the D-017 10 Hz active tier.
const AGENT_STEPS_PER_TICK: usize = 10;
/// Side of the dense agent world, in meters. The sparse profile uses
/// [`DEFAULT_WORLD_SIZE`] (128 m).
const DENSE_WORLD_SIZE: f32 = 32.0;
/// Default measured runs: the docs/68 §6.2 three-run protocol.
const DEFAULT_RUNS: usize = 3;
/// `--smoke` scene: 1,000 cohorts × 100 = 100,000 citizens.
const SMOKE_COHORTS: usize = 1_000;
const SMOKE_TICKS: usize = 60;

/// Bytes per encoded cohort: district, count, employed, six needs, happiness,
/// income. `employment_carry` is private transient state and is outside the
/// pinned checksums, so it is not encoded.
const COHORT_STATE_BYTES: usize = 3 * 4 + 6 * 4 + 2 * 4;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Pinned smoke-scene checksum. It covers the seed, scene shape, 128 district
/// service profiles, 1,000 cohorts, and both 1,000-agent worlds after 60 ticks.
/// A change here means the fixture or the simulation math moved; update the
/// constant only with a matching docs/68 note.
const SMOKE_CHECKSUM: u64 = 0x0386_e0be_4d0b_7f9b;

const USAGE: &str = "\
usage: cohort_1m [--smoke] [--ticks N] [--cohorts N] [--runs N]

  --smoke      1,000 cohorts / 100,000 citizens / 60 ticks, run twice
  --ticks N    measured ticks (default 3600)
  --cohorts N  cohort count, 100 citizens each (default 10000)
  --runs N     measured runs, 1..=9 (default 3)
";

/// Scene parameters. Tests use a smaller configuration of the same scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Config {
    seed: u64,
    cohorts: usize,
    citizens_per_cohort: u32,
    districts: usize,
    agents: usize,
    ticks: usize,
    warmup_ticks: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            seed: SEED,
            cohorts: DEFAULT_COHORTS,
            citizens_per_cohort: CITIZENS_PER_COHORT,
            districts: DEFAULT_DISTRICTS,
            agents: DEFAULT_AGENTS,
            ticks: DEFAULT_TICKS,
            warmup_ticks: WARMUP_TICKS,
        }
    }
}

impl Config {
    fn smoke() -> Self {
        Self {
            cohorts: SMOKE_COHORTS,
            ticks: SMOKE_TICKS,
            ..Self::default()
        }
    }

    fn population(&self) -> u64 {
        self.cohorts as u64 * u64::from(self.citizens_per_cohort)
    }
}

/// Measured state after one scene run.
#[derive(Debug)]
struct Outcome {
    config: Config,
    population: u64,
    employed: u64,
    cohort_count: usize,
    districts: usize,
    agent_count: usize,
    sparse_agents: usize,
    dense_agents: usize,
    average_happiness: f32,
    average_need: f32,
    checksum: u64,
    setup_ms: f64,
    warmup_ms: f64,
    tick_loop_ms: f64,
    total_ms: f64,
    tick_samples: Vec<f64>,
    cohort_samples: Vec<f64>,
    agent_samples: Vec<f64>,
    cohort_state_bytes: usize,
    cohort_state_encode_ms: f64,
    cohort_state_decode_ms: f64,
    cohort_state_roundtrip_equal: bool,
    peak_rss_mb: Option<f64>,
    rss_mb: Option<f64>,
}

/// What to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Full,
    Smoke,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Options {
    mode: Mode,
    config: Config,
    runs: usize,
}

fn main() {
    let options = match parse_args(std::env::args().skip(1)) {
        Ok(Some(options)) => options,
        Ok(None) => return,
        Err(message) => {
            eprintln!("cohort_1m: {message}");
            eprint!("{USAGE}");
            std::process::exit(2);
        }
    };

    match options.mode {
        Mode::Smoke => run_smoke(options.config),
        Mode::Full => run_full(options),
    }
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Option<Options>, String> {
    let mut mode = Mode::Full;
    let mut config = Config::default();
    let mut runs = DEFAULT_RUNS;
    let mut cohorts_override = None;
    let mut ticks_override = None;
    let mut args = args.peekable();

    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--smoke" => mode = Mode::Smoke,
            "--help" | "-h" => {
                print!("{USAGE}");
                return Ok(None);
            }
            "--ticks" => {
                let value = args.next().ok_or("--ticks needs a value")?;
                ticks_override = Some(parse_count("--ticks", &value)?);
            }
            "--cohorts" => {
                let value = args.next().ok_or("--cohorts needs a value")?;
                cohorts_override = Some(parse_count("--cohorts", &value)?);
            }
            "--runs" => {
                let value = args.next().ok_or("--runs needs a value")?;
                let parsed = parse_count("--runs", &value)?;
                if !(1..=9).contains(&parsed) {
                    return Err("--runs must be 1..=9".to_string());
                }
                runs = parsed;
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }

    if mode == Mode::Smoke {
        config = Config::smoke();
    }
    if let Some(cohorts) = cohorts_override {
        if cohorts == 0 {
            return Err("--cohorts must be at least 1".to_string());
        }
        config.cohorts = cohorts;
    }
    if let Some(ticks) = ticks_override {
        if ticks == 0 {
            return Err("--ticks must be at least 1".to_string());
        }
        config.ticks = ticks;
    }

    Ok(Some(Options { mode, config, runs }))
}

fn parse_count(name: &str, value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|_| format!("{name} expects a non-negative integer, got {value:?}"))
}

fn run_full(options: Options) {
    let outcomes: Vec<Outcome> = (0..options.runs)
        .map(|_| simulate(options.config))
        .collect();
    let report = report(&options, &outcomes);
    println!("{}", serde_json::to_string(&report).expect("json"));
    if report["status"] != "ok" {
        std::process::exit(1);
    }
}

/// Run the smoke scene twice and fail unless the state checksum matches.
fn run_smoke(config: Config) {
    let first = simulate(config);
    let second = simulate(config);
    let deterministic = first.checksum == second.checksum
        && first.population == second.population
        && first.employed == second.employed
        && first.average_happiness.to_bits() == second.average_happiness.to_bits()
        && first.average_need.to_bits() == second.average_need.to_bits();
    let ok = deterministic
        && first.population == config.population()
        && first.cohort_count == config.cohorts
        && first.tick_samples.len() == config.ticks
        && (config != Config::smoke() || first.checksum == SMOKE_CHECKSUM);

    println!(
        "COHORT_1M_SMOKE cohorts={} population={} districts={} agents={} \
         ticks={} runs=2 checksums={:016x},{:016x} deterministic={} status={}",
        first.cohort_count,
        first.population,
        first.districts,
        first.agent_count,
        first.config.ticks,
        first.checksum,
        second.checksum,
        deterministic,
        if ok { "ok" } else { "failed" }
    );
    if !ok {
        std::process::exit(1);
    }
}

fn simulate(config: Config) -> Outcome {
    assert!(config.cohorts > 0, "scene needs at least one cohort");
    assert!(config.districts > 0, "scene needs at least one district");
    assert!(
        config.citizens_per_cohort > 0,
        "a cohort needs at least one citizen"
    );

    let scene_start = Instant::now();
    let setup_start = Instant::now();

    let mut city = CohortSimulation::new(config.districts);
    for (index, services) in city.districts_mut().iter_mut().enumerate() {
        *services = district_services(config.seed, index as u32);
    }

    let mut rng = Pcg32::new(config.seed ^ 0x1_0000_0001);
    for index in 0..config.cohorts {
        let needs = 0.35 + (rng.next_u32() % 40) as f32 * 0.01;
        city.add_cohort(Cohort::new(
            (index % config.districts) as u32,
            config.citizens_per_cohort,
            needs,
        ))
        .expect("district exists");
    }

    // Split the D-017 active tier across the two documented profiles so the
    // scene covers both sparse separation and the dense neighbor cap.
    let sparse_agents = config.agents / 2;
    let dense_agents = config.agents - sparse_agents;
    let mut sparse = ActiveAgentSim::with_world_size(
        sparse_agents,
        config.seed ^ 0x5A17_5EED,
        DEFAULT_WORLD_SIZE,
    );
    let mut dense =
        ActiveAgentSim::with_world_size(dense_agents, config.seed ^ 0xD3A5_E5EED, DENSE_WORLD_SIZE);

    let setup_ms = elapsed_ms(setup_start);

    // Warm-up is discarded from every measured distribution.
    let warmup_start = Instant::now();
    for _ in 0..config.warmup_ticks {
        city.tick(1.0);
        for _ in 0..AGENT_STEPS_PER_TICK {
            sparse.tick();
            dense.tick();
        }
    }
    let warmup_ms = elapsed_ms(warmup_start);

    let mut tick_samples = Vec::with_capacity(config.ticks);
    let mut cohort_samples = Vec::with_capacity(config.ticks);
    let mut agent_samples = Vec::with_capacity(config.ticks);

    let loop_start = Instant::now();
    for _ in 0..config.ticks {
        let tick_start = Instant::now();

        // 1 Hz cohort step.
        let cohort_start = Instant::now();
        city.tick(1.0);
        cohort_samples.push(elapsed_ms(cohort_start));

        // 10 Hz active tier: ten steps per 1 Hz city tick.
        let agents_start = Instant::now();
        for _ in 0..AGENT_STEPS_PER_TICK {
            sparse.tick();
            dense.tick();
        }
        agent_samples.push(elapsed_ms(agents_start));

        tick_samples.push(elapsed_ms(tick_start));
    }
    let tick_loop_ms = elapsed_ms(loop_start);

    // Keep the aggregate reads in the measured work.
    let average_happiness = black_box(city.average_happiness());
    let average_need = black_box(city.average_need());
    let population = city.total_population();
    let employed = city.employed_population();
    let checksum = scene_checksum(&config, &city, &sparse, &dense);

    // A deterministic cohort-state round trip. This is not the world save; it
    // proves the encoded cohort state survives encode/decode unchanged.
    let encode_start = Instant::now();
    let encoded = encode_cohorts(&city);
    let cohort_state_encode_ms = elapsed_ms(encode_start);
    let decode_start = Instant::now();
    let decoded = decode_cohorts(&encoded);
    let cohort_state_decode_ms = elapsed_ms(decode_start);
    let cohort_state_roundtrip_equal = cohort_checksum(&decoded) == cohort_checksum(city.cohorts())
        && encoded.len() == city.cohort_count() * COHORT_STATE_BYTES;

    let (peak_rss_mb, rss_mb) = rss_mb();

    Outcome {
        config,
        population,
        employed,
        cohort_count: city.cohort_count(),
        districts: city.districts().len(),
        agent_count: sparse.agent_count() + dense.agent_count(),
        sparse_agents: sparse.agent_count(),
        dense_agents: dense.agent_count(),
        average_happiness,
        average_need,
        checksum,
        setup_ms,
        warmup_ms,
        tick_loop_ms,
        total_ms: elapsed_ms(scene_start),
        tick_samples,
        cohort_samples,
        agent_samples,
        cohort_state_bytes: encoded.len(),
        cohort_state_encode_ms,
        cohort_state_decode_ms,
        cohort_state_roundtrip_equal,
        peak_rss_mb,
        rss_mb,
    }
}

fn report(options: &Options, outcomes: &[Outcome]) -> serde_json::Value {
    let config = options.config;
    let deterministic = outcomes
        .iter()
        .all(|outcome| outcome.checksum == outcomes[0].checksum);
    // The docs/68 three-run protocol reports the median run. Rank runs by tick
    // p99 and take the upper-middle one (for three runs that is the true
    // middle; for an even count it is the slower of the middle pair), so one
    // slow run cannot set the headline.
    let mut ranked: Vec<usize> = (0..outcomes.len()).collect();
    ranked.sort_by(|left, right| {
        percentile(&outcomes[*left].tick_samples, 0.99)
            .partial_cmp(&percentile(&outcomes[*right].tick_samples, 0.99))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let selected_run = ranked[ranked.len() / 2];
    let median_run = &outcomes[selected_run];
    let run_checksums: Vec<String> = outcomes
        .iter()
        .map(|outcome| format!("{:016x}", outcome.checksum))
        .collect();
    let run_p99: Vec<f64> = outcomes
        .iter()
        .map(|outcome| round(percentile(&outcome.tick_samples, 0.99), 4))
        .collect();
    let p99_spread_pct = spread_pct(&run_p99);
    let population_consistent = outcomes
        .iter()
        .all(|outcome| outcome.population == config.population());
    let valid = deterministic
        && population_consistent
        && median_run.cohort_state_roundtrip_equal
        && outcomes
            .iter()
            .all(|outcome| outcome.cohort_state_roundtrip_equal);

    let status = if valid { "ok" } else { "failed" };
    let tick_p99 = percentile(&median_run.tick_samples, 0.99);
    let total = median_run;
    let per_run: Vec<serde_json::Value> = outcomes
        .iter()
        .enumerate()
        .map(|(index, outcome)| {
            serde_json::json!({
                "run": index,
                "selected": index == selected_run,
                "checksum": format!("{:016x}", outcome.checksum),
                "population": outcome.population,
                "employed": outcome.employed,
                "setup_ms": round(outcome.setup_ms, 3),
                "warmup_ms": round(outcome.warmup_ms, 3),
                "tick_loop_ms": round(outcome.tick_loop_ms, 3),
                "total_ms": round(outcome.total_ms, 3),
                "tick_ms_median": round(percentile(&outcome.tick_samples, 0.50), 4),
                "tick_ms_p95": round(percentile(&outcome.tick_samples, 0.95), 4),
                "tick_ms_p99": round(percentile(&outcome.tick_samples, 0.99), 4),
                "tick_ms_max": round(max_sample(&outcome.tick_samples), 4),
                "cohort_tick_ms_p99": round(percentile(&outcome.cohort_samples, 0.99), 4),
                "active_agents_ms_p99": round(percentile(&outcome.agent_samples, 0.99), 4),
                "peak_rss_mb": outcome.peak_rss_mb,
            })
        })
        .collect();
    let tick_mean = mean_sample(&total.tick_samples);
    let cohort_mean = mean_sample(&total.cohort_samples);
    let agent_mean = mean_sample(&total.agent_samples);

    serde_json::json!({
        "format_version": 1,
        "gate": "G5",
        "config": "C2_dedicated_server",
        "scene": "cohort_1m_v1",
        "route": "cohort_3600_ticks_v1",
        "settings": "headless_1hz_v1",
        "duration_s": config.ticks as f64,
        "warmup_s": config.warmup_ticks as f64,
        "runs": outcomes.len(),
        "selected_run": selected_run,
        "headless": true,
        "hardware": hardware(),
        "results": {
            "ticks": config.ticks,
            "cohort_tick_ms_p99": round(percentile(&total.cohort_samples, 0.99), 4),
            "full_tick_ms_p99": round(tick_p99, 4),
            "active_agents_ms_p99": round(percentile(&total.agent_samples, 0.99), 4),
            "sim_cpu_ms_per_s": round(tick_p99, 4),
            "server_rss_mb_peak": total.peak_rss_mb,
            "valid": valid,
            "tick_ms_median": round(percentile(&total.tick_samples, 0.50), 4),
            "tick_ms_p95": round(percentile(&total.tick_samples, 0.95), 4),
            "tick_ms_p99": round(tick_p99, 4),
            "tick_ms_max": round(max_sample(&total.tick_samples), 4),
            "tick_ms_mean": round(tick_mean, 4),
            "cohort_tick_ms_median": round(percentile(&total.cohort_samples, 0.50), 4),
            "cohort_tick_ms_p95": round(percentile(&total.cohort_samples, 0.95), 4),
            "cohort_tick_ms_p99": round(percentile(&total.cohort_samples, 0.99), 4),
            "cohort_tick_ms_max": round(max_sample(&total.cohort_samples), 4),
            "cohort_tick_ms_mean": round(cohort_mean, 4),
            "active_agents_ms_median": round(percentile(&total.agent_samples, 0.50), 4),
            "active_agents_ms_p95": round(percentile(&total.agent_samples, 0.95), 4),
            "active_agents_ms_p99": round(percentile(&total.agent_samples, 0.99), 4),
            "active_agents_ms_max": round(max_sample(&total.agent_samples), 4),
            "active_agents_ms_mean": round(agent_mean, 4),
            "sim_cpu_ms_per_s_mean": round(tick_mean, 4),
            "cohort_state_bytes": total.cohort_state_bytes,
            "cohort_state_encode_ms": round(total.cohort_state_encode_ms, 4),
            "cohort_state_decode_ms": round(total.cohort_state_decode_ms, 4),
            "cohort_state_roundtrip_equal": total.cohort_state_roundtrip_equal,
            "run_checksums": run_checksums,
            "checksums_match": deterministic,
            "run_tick_p99_ms": run_p99,
            "tick_p99_spread_pct": round(p99_spread_pct, 4),
            "checksum": format!("{:016x}", total.checksum),
        },
        "seed": config.seed,
        "population": total.population,
        "cohorts": total.cohort_count,
        "citizens_per_cohort": config.citizens_per_cohort,
        "districts": total.districts,
        "active_agents": total.agent_count,
        "sparse_agents": total.sparse_agents,
        "dense_agents": total.dense_agents,
        "ticks": config.ticks,
        "warmup_ticks": config.warmup_ticks,
        "deterministic": deterministic,
        "checksum": format!("{:016x}", total.checksum),
        "checksum_scope": "seed, scene shape, district services, every cohort, and both active-agent worlds after the final tick",
        "per_run": per_run,
        "employed": total.employed,
        "average_happiness": total.average_happiness,
        "average_need": total.average_need,
        "peak_rss_mb": total.peak_rss_mb,
        "rss_mb": total.rss_mb,
        "rss_source": "/proc/self/status VmHWM (peak) and VmRSS (final)",
        "setup_ms": round(median_run.setup_ms, 3),
        "warmup_ms": round(median_run.warmup_ms, 3),
        "tick_loop_ms": round(median_run.tick_loop_ms, 3),
        "total_ms": round(median_run.total_ms, 3),
        "status": status,
        "evidence_scope": "Headless cohort + active-agent tick evidence only. No renderer, world, buildings, economy, traffic, pathfinding, network, or world save is measured. The gate evaluator reports insufficient until the full server record exists.",
        "note": "cohort_tick = CohortSimulation::tick(1.0). full_tick = the whole 1 Hz tick (cohort step + ten 10 Hz agent steps); this harness has no district/building/economy aggregate, so full_tick is a conservative superset of the cohort step. sim_cpu_ms_per_s uses the tick p99 because one measured tick advances one simulated second. server_rss_mb_peak is this single headless process's peak RSS, not the townsmith-server crate. pathfinding, traffic, network, and save/load fields are omitted because the scene does not measure them; cohort_state_* is a 44-byte-per-cohort round trip, not the 1M world save.",
    })
}

/// Fixed, seed-derived service profile for one district.
///
/// The profile is static for the whole run: this scene has no policy drift,
/// building stock, or economy loop. Every value stays inside `0..=1`.
fn district_services(seed: u64, index: u32) -> DistrictServices {
    let mut rng = Pcg32::new(seed ^ 0xD157_71C7 ^ u64::from(index));
    let mut next = || rng.next_f32();
    DistrictServices {
        food: 0.85 + 0.10 * next(),
        water: 0.88 + 0.10 * next(),
        rest: 0.70 + 0.20 * next(),
        hygiene: 0.75 + 0.15 * next(),
        social: 0.60 + 0.25 * next(),
        safety: 0.75 + 0.20 * next(),
        jobs: 0.78 + 0.17 * next(),
        housing: 0.75 + 0.20 * next(),
        pollution: 0.05 + 0.10 * next(),
        noise: 0.05 + 0.08 * next(),
        commute: 0.05 + 0.15 * next(),
        wages: 0.90 + 0.20 * next(),
    }
}

fn scene_checksum(
    config: &Config,
    city: &CohortSimulation,
    sparse: &ActiveAgentSim,
    dense: &ActiveAgentSim,
) -> u64 {
    let mut state = FNV_OFFSET;

    state = fnv_u64(state, 0x0043_4F48_4F52_5431); // "COHORT1"
    state = fnv_u64(state, config.seed);
    state = fnv_u64(state, config.cohorts as u64);
    state = fnv_u64(state, u64::from(config.citizens_per_cohort));
    state = fnv_u64(state, config.districts as u64);
    state = fnv_u64(state, config.ticks as u64);

    state = fnv_u64(state, 0x4449_5354_5249_4354); // "DISTRICT"
    for services in city.districts() {
        for value in [
            services.food,
            services.water,
            services.rest,
            services.hygiene,
            services.social,
            services.safety,
            services.jobs,
            services.housing,
            services.pollution,
            services.noise,
            services.commute,
            services.wages,
        ] {
            state = fnv_u64(state, u64::from(value.to_bits()));
        }
    }

    state = fnv_u64(state, 0x0043_4F48_4F52_5453); // "COHORTS"
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

    state
}

/// Encode the public cohort state, 44 bytes per cohort.
fn encode_cohorts(city: &CohortSimulation) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(city.cohort_count() * COHORT_STATE_BYTES);
    for cohort in city.cohorts() {
        bytes.extend_from_slice(&cohort.district.to_le_bytes());
        bytes.extend_from_slice(&cohort.count.to_le_bytes());
        bytes.extend_from_slice(&cohort.employed.to_le_bytes());
        for need in cohort.needs {
            bytes.extend_from_slice(&need.to_bits().to_le_bytes());
        }
        bytes.extend_from_slice(&cohort.happiness.to_bits().to_le_bytes());
        bytes.extend_from_slice(&cohort.income.to_bits().to_le_bytes());
    }
    bytes
}

fn decode_cohorts(bytes: &[u8]) -> Vec<Cohort> {
    let mut cohorts = Vec::with_capacity(bytes.len() / COHORT_STATE_BYTES);
    for chunk in bytes.as_chunks::<COHORT_STATE_BYTES>().0 {
        let district = u32::from_le_bytes(chunk[0..4].try_into().expect("district"));
        let count = u32::from_le_bytes(chunk[4..8].try_into().expect("count"));
        let employed = u32::from_le_bytes(chunk[8..12].try_into().expect("employed"));
        let mut cohort = Cohort::new(district, count, 0.0);
        for (index, need) in cohort.needs.iter_mut().enumerate() {
            let offset = 12 + index * 4;
            let bits = u32::from_le_bytes(chunk[offset..offset + 4].try_into().expect("need"));
            *need = f32::from_bits(bits);
        }
        let happiness_offset = 12 + 6 * 4;
        let bits = u32::from_le_bytes(
            chunk[happiness_offset..happiness_offset + 4]
                .try_into()
                .expect("happiness"),
        );
        cohort.happiness = f32::from_bits(bits);
        let income_offset = happiness_offset + 4;
        let bits = u32::from_le_bytes(
            chunk[income_offset..income_offset + 4]
                .try_into()
                .expect("income"),
        );
        cohort.income = f32::from_bits(bits);
        cohort.employed = employed;
        cohorts.push(cohort);
    }
    debug_assert_eq!(bytes.len() % COHORT_STATE_BYTES, 0);
    cohorts
}

fn cohort_checksum(cohorts: &[Cohort]) -> u64 {
    let mut state = FNV_OFFSET;
    state = fnv_u64(state, cohorts.len() as u64);
    for cohort in cohorts {
        state = fnv_u64(state, u64::from(cohort.district));
        state = fnv_u64(state, u64::from(cohort.count));
        state = fnv_u64(state, u64::from(cohort.employed));
        for need in cohort.needs {
            state = fnv_u64(state, u64::from(need.to_bits()));
        }
        state = fnv_u64(state, u64::from(cohort.happiness.to_bits()));
        state = fnv_u64(state, u64::from(cohort.income.to_bits()));
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

fn mean_sample(samples: &[f64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.iter().sum::<f64>() / samples.len() as f64
}

fn spread_pct(values: &[f64]) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    if mean <= 0.0 {
        return 0.0;
    }
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    (max - min) / mean * 100.0
}

fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn round(value: f64, places: u32) -> f64 {
    let factor = 10.0_f64.powi(places as i32);
    (value * factor).round() / factor
}

/// Peak and final resident set in MiB from `/proc/self/status`, best effort.
fn rss_mb() -> (Option<f64>, Option<f64>) {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return (None, None);
    };
    let read = |name: &str| -> Option<f64> {
        let line = status.lines().find(|line| line.starts_with(name))?;
        let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(round(kb / 1024.0, 2))
    };
    (read("VmHWM:"), read("VmRSS:"))
}

/// Best-effort hardware provenance for the gate schema (CPU and RAM only; the
/// headless scene has no GPU or display requirement).
fn hardware() -> serde_json::Value {
    let (cpu, cores, threads) = cpu_info();
    let ram_gb = system_ram_gb();
    let os = os_name();
    serde_json::json!({
        "cpu": cpu,
        "cpu_cores": cores,
        "cpu_threads": threads,
        "ram_gb": ram_gb,
        "os": os,
    })
}

fn cpu_info() -> (Option<String>, Option<usize>, Option<usize>) {
    let Ok(text) = std::fs::read_to_string("/proc/cpuinfo") else {
        return (None, None, None);
    };
    let mut model: Option<String> = None;
    let mut threads = 0usize;
    let mut pairs: BTreeSet<(u32, u32)> = BTreeSet::new();
    let mut declared_cores: Option<usize> = None;

    for block in text.split("\n\n") {
        let mut has_processor = false;
        let mut physical: Option<u32> = None;
        let mut core: Option<u32> = None;
        for line in block.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "processor" => has_processor = true,
                "model name" | "Processor" if model.is_none() => {
                    model = Some(value.to_string());
                }
                "physical id" => physical = value.parse().ok(),
                "core id" => core = value.parse().ok(),
                "cpu cores" if declared_cores.is_none() => {
                    declared_cores = value.parse().ok();
                }
                _ => {}
            }
        }
        if has_processor {
            threads += 1;
            if let (Some(physical), Some(core)) = (physical, core) {
                pairs.insert((physical, core));
            }
        }
    }

    let cores = if pairs.is_empty() {
        declared_cores
    } else {
        Some(pairs.len())
    };
    (
        model,
        cores,
        if threads == 0 { None } else { Some(threads) },
    )
}

fn system_ram_gb() -> Option<f64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = text.lines().find(|line| line.starts_with("MemTotal:"))?;
    let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(round(kb / (1024.0 * 1024.0), 2))
}

fn os_name() -> String {
    if let Ok(text) = std::fs::read_to_string("/etc/os-release") {
        if let Some(line) = text.lines().find(|line| line.starts_with("PRETTY_NAME=")) {
            let value = line
                .trim_start_matches("PRETTY_NAME=")
                .trim_matches('"')
                .to_string();
            if !value.is_empty() {
                return value;
            }
        }
    }
    format!("{} ({})", std::env::consts::OS, std::env::consts::ARCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_config() -> Config {
        Config {
            cohorts: 64,
            districts: 4,
            agents: 16,
            ticks: 4,
            warmup_ticks: 1,
            ..Config::default()
        }
    }

    #[test]
    fn default_counts_match_the_gate_scene() {
        let config = Config::default();
        assert_eq!(config.cohorts, 10_000);
        assert_eq!(config.citizens_per_cohort, 100);
        assert_eq!(config.population(), 1_000_000);
        assert_eq!(config.districts, 128);
        assert_eq!(config.agents, 2_000);
        assert_eq!(config.ticks, 3_600);
        assert_eq!(DEFAULT_RUNS, 3);
    }

    #[test]
    fn smoke_fixture_is_100k_citizens_for_60_ticks() {
        let config = Config::smoke();
        assert_eq!(config.cohorts, 1_000);
        assert_eq!(config.population(), 100_000);
        assert_eq!(config.ticks, 60);
        assert_eq!(config.districts, 128);
        assert_eq!(config.agents, 2_000);
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
        assert_eq!(first.districts, 4);
        assert_eq!(first.agent_count, 16);
        assert_eq!(first.tick_samples.len(), 4);
        assert!(first.cohort_state_roundtrip_equal);
        assert_eq!(first.cohort_state_bytes, 64 * COHORT_STATE_BYTES);
    }

    #[test]
    fn different_seeds_change_the_checksum() {
        let first = simulate(small_config());
        let second = simulate(Config {
            seed: SEED ^ 0xFFFF,
            ..small_config()
        });
        assert_ne!(first.checksum, second.checksum);
    }

    #[test]
    fn district_profiles_are_deterministic_and_bounded() {
        for index in 0..128 {
            let first = district_services(SEED, index);
            assert_eq!(first, district_services(SEED, index));
            for value in [
                first.food,
                first.water,
                first.rest,
                first.hygiene,
                first.social,
                first.safety,
                first.jobs,
                first.housing,
                first.pollution,
                first.noise,
                first.commute,
            ] {
                assert!((0.0..=1.0).contains(&value), "district {index}: {value}");
            }
            // Wages are an income multiplier, not a 0..=1 service fraction;
            // `DistrictServices::sanitized` clamps them to 0..=10.
            assert!(
                (0.0..=10.0).contains(&first.wages),
                "district {index}: wages {}",
                first.wages
            );
            assert_eq!(first.sanitized(), first, "profile must already be sane");
        }
        assert_ne!(district_services(SEED, 0), district_services(SEED, 1));
    }

    #[test]
    fn cohort_state_round_trip_is_exact() {
        let mut city = CohortSimulation::new(2);
        for index in 0..8 {
            city.add_cohort(Cohort::new(
                (index % 2) as u32,
                100,
                0.4 + index as f32 * 0.01,
            ))
            .expect("district exists");
        }
        city.tick(1.0);
        let encoded = encode_cohorts(&city);
        assert_eq!(encoded.len(), 8 * COHORT_STATE_BYTES);
        let decoded = decode_cohorts(&encoded);
        assert_eq!(decoded.len(), 8);
        assert_eq!(cohort_checksum(&decoded), cohort_checksum(city.cohorts()));
        // The private employment carry is transient and outside the pinned
        // checksums, so the round trip compares the public state checksum.
        for (decoded, original) in decoded.iter().zip(city.cohorts()) {
            assert_eq!(decoded.district, original.district);
            assert_eq!(decoded.count, original.count);
            assert_eq!(decoded.employed, original.employed);
            assert_eq!(decoded.needs, original.needs);
            assert_eq!(decoded.happiness, original.happiness);
            assert_eq!(decoded.income, original.income);
        }
    }

    #[test]
    fn report_carries_the_gate_fields() {
        let options = Options {
            mode: Mode::Full,
            config: small_config(),
            runs: 2,
        };
        let outcomes = vec![simulate(options.config), simulate(options.config)];
        let report = report(&options, &outcomes);
        assert_eq!(report["scene"], "cohort_1m_v1");
        assert_eq!(report["headless"], true);
        assert_eq!(report["runs"], 2);
        assert_eq!(report["results"]["ticks"], 4);
        assert_eq!(report["population"], 6_400);
        assert_eq!(report["cohorts"], 64);
        assert_eq!(report["districts"], 4);
        assert!(report["results"]["cohort_tick_ms_p99"].is_number());
        assert!(report["results"]["full_tick_ms_p99"].is_number());
        assert!(report["results"]["active_agents_ms_p99"].is_number());
        assert!(report["results"]["sim_cpu_ms_per_s"].is_number());
        assert_eq!(report["results"]["valid"], true);
        assert_eq!(report["deterministic"], true);
        assert_eq!(report["status"], "ok");
        assert_eq!(report["runs"], 2);
        assert_eq!(report["results"]["checksums_match"], true);
        let per_run = report["per_run"].as_array().expect("per_run");
        assert_eq!(per_run.len(), 2);
        assert_eq!(per_run[0]["checksum"], per_run[1]["checksum"]);
        assert_eq!(per_run[0]["checksum"], report["checksum"]);
        // The selected run is the slower of the middle pair by tick p99, so
        // which run wins depends on machine noise. Assert the invariant, not
        // a fixed index.
        let selected = report["selected_run"].as_u64().expect("selected_run") as usize;
        assert!(selected < per_run.len());
        assert_eq!(
            per_run.iter().filter(|run| run["selected"] == true).count(),
            1
        );
        assert_eq!(per_run[selected]["checksum"], report["checksum"]);
        assert_eq!(per_run[selected]["selected"], true);
        assert!(per_run[0]["tick_ms_median"].is_number());
        assert!(per_run[0]["tick_ms_p95"].is_number());
        assert!(per_run[0]["tick_ms_p99"].is_number());
        assert!(per_run[0]["tick_ms_max"].is_number());
        // The harness must not claim gate fields it does not measure.
        assert!(report["results"].get("save_ms").is_none());
        assert!(report["results"].get("save_bytes").is_none());
        assert!(report["results"].get("pathfinding_ms_p99").is_none());
        assert!(report["results"].get("traffic_build_ms").is_none());
        assert!(report["results"].get("network_stall_ms_max").is_none());
        assert!(report["checksum"]
            .as_str()
            .is_some_and(|value| value.len() == 16));
    }

    #[test]
    fn report_fails_when_runs_disagree() {
        let options = Options {
            mode: Mode::Full,
            config: small_config(),
            runs: 2,
        };
        let first = simulate(options.config);
        let mut second = simulate(options.config);
        second.checksum ^= 1;
        let report = report(&options, &[first, second]);
        assert_eq!(report["deterministic"], false);
        assert_eq!(report["results"]["checksums_match"], false);
        assert_eq!(report["results"]["valid"], false);
        assert_eq!(report["status"], "failed");
    }

    #[test]
    fn smoke_checksum_is_pinned() {
        let outcome = simulate(Config::smoke());
        assert_eq!(outcome.checksum, SMOKE_CHECKSUM);
        assert_eq!(outcome.population, 100_000);
        assert_eq!(outcome.cohort_count, 1_000);
    }
}
