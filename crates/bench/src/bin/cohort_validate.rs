//! Cohort validation harness: aggregate vs individual, and coarse vs fine.
//!
//! This is the small-city reference check from `docs/07 - Simulation and AI.md`
//! §14. It answers two separate questions and reports them separately.
//!
//! 1. **Aggregate vs individual.** One 500-citizen district is simulated as
//!    5 cohorts × 100 citizens and as 500 cohorts × 1 citizen. The need and
//!    happiness rules are per citizen and linear, so both representations must
//!    land on the same state. The strict comparison uses a fully-employed
//!    profile (`jobs = 1.0`), because the integer employment rule is not
//!    count-invariant: `round(count * jobs)` rounds a one-citizen cohort to
//!    either 0 or 1 employed. That structural difference is reported in the
//!    `structural` block, not hidden behind a loose gate on the strict check.
//! 2. **Coarse vs fine.** The mixed profile (`jobs = 0.9`) runs for 365 days
//!    with 1-day steps and with 1/24-day steps. The exponential need and
//!    happiness filters converge to the same fixed point at both step sizes;
//!    integer employment does not, because a 1/24-day employment step rounds
//!    back to the current value. The measured divergence is reported, along
//!    with the documented bound, as evidence for dormant catch-up.
//!
//! The output is one JSON object with exact deltas, a deterministic FNV
//! checksum over every run, and a `status`. `--smoke` runs the default scene
//! twice and fails on any checksum or delta drift. There are no sleeps and no
//! wall-clock measurements.
//!
//! See `docs/07 - Simulation and AI.md` §14 and `docs/68 - Metropolis Gate.md`
//! §7.8.

use core_sim::{Cohort, CohortSimulation, DistrictServices};

/// Fixed seed for the scene. It selects the initial needs level and salts the
/// checksum; no random process runs during the simulation.
const SEED: u64 = 0x434F_4856_414C; // "COHVAL"
/// Simulated days per run.
const DEFAULT_DAYS: usize = 365;
/// Cohorts in the aggregate city.
const COHORTS: usize = 5;
/// Citizens per aggregate cohort.
const CITIZENS_PER_COHORT: u32 = 100;
/// Fine steps per simulated day.
const FINE_STEPS_PER_DAY: usize = 24;
/// Gate for the strict aggregate-vs-individual comparison.
const STRICT_TOLERANCE: f64 = 1e-4;
/// Bound on the employment-rounding divergence (8% of the population at
/// `jobs = 0.9`). This is a regression bound, not a correctness bound.
const EMPLOYMENT_BOUND: f64 = 0.10;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Scene parameters. Tests use a shorter `days`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Config {
    seed: u64,
    days: usize,
    cohorts: usize,
    citizens_per_cohort: u32,
    fine_steps_per_day: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            seed: SEED,
            days: DEFAULT_DAYS,
            cohorts: COHORTS,
            citizens_per_cohort: CITIZENS_PER_COHORT,
            fine_steps_per_day: FINE_STEPS_PER_DAY,
        }
    }
}

impl Config {
    /// Population of the scene, in citizens.
    fn population(&self) -> u64 {
        self.cohorts as u64 * u64::from(self.citizens_per_cohort)
    }

    /// Number of one-citizen cohorts in the individual reference.
    fn individual_cohorts(&self) -> usize {
        self.cohorts * self.citizens_per_cohort as usize
    }
}

/// The four comparable city metrics.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Metrics {
    population: u64,
    employed: u64,
    happiness: f32,
    average_need: f32,
}

impl Metrics {
    /// Employed fraction, 0..=1.
    fn employed_fraction(&self) -> f64 {
        if self.population == 0 {
            return 0.0;
        }
        self.employed as f64 / self.population as f64
    }
}

/// Exact deltas between two metrics snapshots.
///
/// Counts are reported as absolute citizens and as population fractions; the
/// two 0..=1 metrics are absolute differences. [`Deltas::max_delta`] takes the
/// largest of the four normalized values.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Deltas {
    population: u64,
    employed: u64,
    population_fraction: f64,
    employed_fraction: f64,
    happiness: f64,
    average_need: f64,
}

impl Deltas {
    /// Largest normalized divergence across population, employment, happiness,
    /// and average need.
    fn max_delta(&self) -> f64 {
        self.population_fraction
            .max(self.employed_fraction)
            .max(self.happiness)
            .max(self.average_need)
    }
}

/// Final state of every run in the scene.
#[derive(Debug)]
struct Outcome {
    config: Config,
    needs0: f32,
    /// 5 × 100 with `jobs = 1.0`.
    strict_aggregate: Metrics,
    /// 500 × 1 with `jobs = 1.0`.
    strict_individual: Metrics,
    strict_deltas: Deltas,
    /// 5 × 100 with `jobs = 0.9`, 1-day steps.
    coarse: Metrics,
    /// 5 × 100 with `jobs = 0.9`, 1/24-day steps.
    fine: Metrics,
    dt_deltas: Deltas,
    /// 500 × 1 with `jobs = 0.9`, 1-day steps.
    structural_individual: Metrics,
    structural_deltas: Deltas,
    /// Largest per-citizen difference between an aggregate cohort and its
    /// matching 100-citizen group in the individual reference.
    per_cohort_max_delta: f64,
    checksum: u64,
    ok: bool,
}

fn main() {
    if std::env::args().any(|arg| arg == "--smoke") {
        run_smoke();
        return;
    }

    let outcome = simulate(Config::default());
    println!(
        "{}",
        serde_json::to_string(&report(&outcome)).expect("json")
    );
    if !outcome.ok {
        std::process::exit(1);
    }
}

/// Run the default scene twice and fail on any state drift.
fn run_smoke() {
    let first = simulate(Config::default());
    let second = simulate(Config::default());
    let deterministic = first.checksum == second.checksum
        && same_deltas(&first.strict_deltas, &second.strict_deltas)
        && same_deltas(&first.dt_deltas, &second.dt_deltas)
        && same_deltas(&first.structural_deltas, &second.structural_deltas)
        && same_metrics(&first.strict_aggregate, &second.strict_aggregate)
        && same_metrics(&first.fine, &second.fine);
    let ok = deterministic && first.ok;

    println!(
        concat!(
            "COHORT_VALIDATE_SMOKE days={} population={} ",
            "strict_max_delta={:.3e} per_cohort_max_delta={:.3e} ",
            "dt_max_delta={:.6} structural_max_delta={:.6} ",
            "checksum={:016x} deterministic={} status={}"
        ),
        first.config.days,
        first.strict_aggregate.population,
        first.strict_deltas.max_delta(),
        first.per_cohort_max_delta,
        first.dt_deltas.max_delta(),
        first.structural_deltas.max_delta(),
        first.checksum,
        deterministic,
        if ok { "ok" } else { "failed" }
    );
    if !ok {
        std::process::exit(1);
    }
}

/// Build and run every city in the scene.
fn simulate(config: Config) -> Outcome {
    let needs0 = initial_need(config.seed);
    let mixed = mixed_profile();
    let linear = linear_profile();

    let mut strict_aggregate_city =
        build_city(&linear, config.cohorts, config.citizens_per_cohort, needs0);
    let strict_aggregate = run(&mut strict_aggregate_city, config.days, 1);

    let mut strict_individual_city = build_city(&linear, config.individual_cohorts(), 1, needs0);
    let strict_individual = run(&mut strict_individual_city, config.days, 1);

    let mut coarse_city = build_city(&mixed, config.cohorts, config.citizens_per_cohort, needs0);
    let coarse = run(&mut coarse_city, config.days, 1);

    // A separate copy: the coarse run must not be advanced by the fine loop.
    let mut fine_city = build_city(&mixed, config.cohorts, config.citizens_per_cohort, needs0);
    let fine = run(&mut fine_city, config.days, config.fine_steps_per_day);

    let mut structural_individual_city = build_city(&mixed, config.individual_cohorts(), 1, needs0);
    let structural_individual = run(&mut structural_individual_city, config.days, 1);

    let strict_deltas = compare(&strict_individual, &strict_aggregate);
    let dt_deltas = compare(&coarse, &fine);
    let structural_deltas = compare(&structural_individual, &coarse);
    let per_cohort_max_delta = per_cohort_max_delta(
        &strict_aggregate_city,
        &strict_individual_city,
        config.citizens_per_cohort as usize,
    );

    let checksum = state_checksum(
        &config,
        &[
            &strict_aggregate_city,
            &strict_individual_city,
            &coarse_city,
            &fine_city,
            &structural_individual_city,
        ],
    );

    let ok = strict_deltas.max_delta() <= STRICT_TOLERANCE
        && strict_deltas.population == 0
        && strict_aggregate.population == config.population()
        && per_cohort_max_delta <= STRICT_TOLERANCE
        && dt_deltas.max_delta() <= EMPLOYMENT_BOUND
        && dt_deltas.population == 0
        && structural_deltas.max_delta() <= EMPLOYMENT_BOUND
        && structural_deltas.population == 0;

    Outcome {
        config,
        needs0,
        strict_aggregate,
        strict_individual,
        strict_deltas,
        coarse,
        fine,
        dt_deltas,
        structural_individual,
        structural_deltas,
        per_cohort_max_delta,
        checksum,
        ok,
    }
}

/// The fixed mixed service profile: normal services with 90% job coverage.
fn mixed_profile() -> DistrictServices {
    DistrictServices {
        food: 0.9,
        water: 0.95,
        rest: 0.8,
        hygiene: 0.85,
        social: 0.7,
        safety: 0.85,
        jobs: 0.9,
        housing: 0.85,
        pollution: 0.1,
        noise: 0.1,
        commute: 0.15,
        wages: 1.0,
    }
}

/// The mixed profile with full employment. Every cohort, however small, can
/// employ all of its citizens, so the integer employment rule does not move.
fn linear_profile() -> DistrictServices {
    DistrictServices {
        jobs: 1.0,
        ..mixed_profile()
    }
}

/// Deterministic initial need level, in `0.35..=0.74`.
///
/// The same value seeds every city in the scene, so the aggregate and the
/// individual reference start from an identical state.
fn initial_need(seed: u64) -> f32 {
    0.35 + (seed % 40) as f32 * 0.01
}

/// Build a single-district city with `cohorts` cohorts of `citizens` each.
fn build_city(
    profile: &DistrictServices,
    cohorts: usize,
    citizens: u32,
    needs: f32,
) -> CohortSimulation {
    let mut city = CohortSimulation::new(1);
    city.districts_mut()[0] = *profile;
    for _ in 0..cohorts {
        city.add_cohort(Cohort::new(0, citizens, needs))
            .expect("district exists");
    }
    city
}

/// Advance `city` by `days` days in `steps_per_day` equal steps.
fn run(city: &mut CohortSimulation, days: usize, steps_per_day: usize) -> Metrics {
    let steps = days.saturating_mul(steps_per_day);
    let dt_days = 1.0 / steps_per_day as f32;
    for _ in 0..steps {
        city.tick(dt_days);
    }
    metrics(city)
}

fn metrics(city: &CohortSimulation) -> Metrics {
    Metrics {
        population: city.total_population(),
        employed: city.employed_population(),
        happiness: city.average_happiness(),
        average_need: city.average_need(),
    }
}

/// Exact divergence of `other` from `reference`.
fn compare(reference: &Metrics, other: &Metrics) -> Deltas {
    let population_delta = reference.population.abs_diff(other.population);
    let population_base = reference.population.max(1) as f64;
    Deltas {
        population: population_delta,
        employed: reference.employed.abs_diff(other.employed),
        population_fraction: population_delta as f64 / population_base,
        employed_fraction: (reference.employed_fraction() - other.employed_fraction()).abs(),
        happiness: (f64::from(reference.happiness) - f64::from(other.happiness)).abs(),
        average_need: (f64::from(reference.average_need) - f64::from(other.average_need)).abs(),
    }
}

fn report(outcome: &Outcome) -> serde_json::Value {
    let config = outcome.config;
    serde_json::json!({
        "scene": "cohort_validate",
        "format_version": 1,
        "seed": config.seed,
        "days": config.days,
        "cohorts": config.cohorts,
        "individual_cohorts": config.individual_cohorts(),
        "citizens_per_cohort": config.citizens_per_cohort,
        "population": outcome.strict_aggregate.population,
        "initial_need": outcome.needs0,
        "aggregate": metrics_json(&outcome.strict_aggregate),
        "individual": metrics_json(&outcome.strict_individual),
        "deltas": deltas_json(&outcome.strict_deltas),
        "max_delta": outcome.strict_deltas.max_delta(),
        "tolerance": STRICT_TOLERANCE,
        "per_cohort_max_delta": outcome.per_cohort_max_delta,
        "note": "Per-cohort need, happiness, and income state is identical; the run-level residual is f32 summation order in the population-weighted reduction.",
        "profile": "linear: jobs=1.0, food=0.9, water=0.95, rest=0.8, hygiene=0.85, social=0.7, safety=0.85, housing=0.85, pollution=0.1, noise=0.1, commute=0.15, wages=1.0",
        "dt": {
            "coarse_dt_days": 1.0,
            "fine_dt_days": 1.0 / config.fine_steps_per_day as f64,
            "fine_steps_per_day": config.fine_steps_per_day,
            "profile": "mixed: jobs=0.9",
            "coarse": metrics_json(&outcome.coarse),
            "fine": metrics_json(&outcome.fine),
            "deltas": deltas_json(&outcome.dt_deltas),
            "max_delta": outcome.dt_deltas.max_delta(),
            "tolerance": EMPLOYMENT_BOUND,
            "step_invariant": outcome.dt_deltas.max_delta() <= STRICT_TOLERANCE,
            "within_tolerance": outcome.dt_deltas.max_delta() <= EMPLOYMENT_BOUND,
            "divergence_cause": "integer employment: a 1/24-day step rounds back to the current employed count",
        },
        "structural": {
            "note": "500 x 1 with jobs=0.9: round(1 * 0.9) = 1, so every one-citizen cohort stays fully employed while the 100-citizen cohorts converge toward 90 jobs. Needs are still identical.",
            "profile": "mixed: jobs=0.9, 1-day steps",
            "aggregate": metrics_json(&outcome.coarse),
            "individual": metrics_json(&outcome.structural_individual),
            "deltas": deltas_json(&outcome.structural_deltas),
            "max_delta": outcome.structural_deltas.max_delta(),
            "tolerance": EMPLOYMENT_BOUND,
        },
        "checksum": format!("{:016x}", outcome.checksum),
        "checksum_scope": "seed + days + every cohort of all five final city states",
        "status": if outcome.ok { "ok" } else { "failed" },
    })
}

fn metrics_json(metrics: &Metrics) -> serde_json::Value {
    serde_json::json!({
        "population": metrics.population,
        "employed": metrics.employed,
        "employed_fraction": metrics.employed_fraction(),
        "happiness": metrics.happiness,
        "average_need": metrics.average_need,
    })
}

fn deltas_json(deltas: &Deltas) -> serde_json::Value {
    serde_json::json!({
        "population": deltas.population,
        "employed": deltas.employed,
        "population_fraction": deltas.population_fraction,
        "employed_fraction": deltas.employed_fraction,
        "happiness": deltas.happiness,
        "average_need": deltas.average_need,
        "max_delta": deltas.max_delta(),
    })
}

/// Largest per-citizen difference between each aggregate cohort and the first
/// cohort of its matching 100-citizen group in the individual reference.
///
/// The run-level metrics go through a population-weighted f32 reduction whose
/// summation order differs between 5 and 500 cohorts; this comparison sees the
/// actual per-citizen state and must be zero.
fn per_cohort_max_delta(
    aggregate: &CohortSimulation,
    individual: &CohortSimulation,
    group: usize,
) -> f64 {
    let mut max_delta = 0.0_f64;
    for (index, cohort) in aggregate.cohorts().iter().enumerate() {
        let reference = &individual.cohorts()[index * group];
        for (need, reference_need) in cohort.needs.iter().zip(reference.needs) {
            max_delta = max_delta.max(f64::from((need - reference_need).abs()));
        }
        max_delta = max_delta.max(f64::from((cohort.happiness - reference.happiness).abs()));
        max_delta = max_delta.max(f64::from((cohort.income - reference.income).abs()));
    }
    max_delta
}

/// FNV-1a over the seed, the day count, and every cohort of every final city.
fn state_checksum(config: &Config, cities: &[&CohortSimulation]) -> u64 {
    let mut state = FNV_OFFSET;
    state = fnv_u64(state, 0x434F_4856_414C); // "COHVAL"
    state = fnv_u64(state, config.seed);
    state = fnv_u64(state, config.days as u64);
    for city in cities {
        state = fnv_u64(state, city.cohort_count() as u64);
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

fn same_metrics(left: &Metrics, right: &Metrics) -> bool {
    left.population == right.population
        && left.employed == right.employed
        && left.happiness.to_bits() == right.happiness.to_bits()
        && left.average_need.to_bits() == right.average_need.to_bits()
}

fn same_deltas(left: &Deltas, right: &Deltas) -> bool {
    left.population == right.population
        && left.employed == right.employed
        && left.population_fraction.to_bits() == right.population_fraction.to_bits()
        && left.employed_fraction.to_bits() == right.employed_fraction.to_bits()
        && left.happiness.to_bits() == right.happiness.to_bits()
        && left.average_need.to_bits() == right.average_need.to_bits()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small() -> Config {
        Config {
            days: 30,
            ..Config::default()
        }
    }

    #[test]
    fn fixture_shapes_match_the_scene() {
        let config = Config::default();
        assert_eq!(config.population(), 500);
        assert_eq!(config.individual_cohorts(), 500);

        let needs = initial_need(config.seed);
        let aggregate = build_city(
            &linear_profile(),
            config.cohorts,
            config.citizens_per_cohort,
            needs,
        );
        let individual = build_city(&linear_profile(), config.individual_cohorts(), 1, needs);

        assert_eq!(aggregate.cohort_count(), 5);
        assert_eq!(individual.cohort_count(), 500);
        assert_eq!(aggregate.total_population(), 500);
        assert_eq!(individual.total_population(), 500);
        assert_eq!(aggregate.cohorts()[0].count, 100);
        assert_eq!(individual.cohorts()[0].count, 1);
        assert_eq!(aggregate.districts(), individual.districts());
        assert_eq!(aggregate.districts()[0].jobs, 1.0);
        assert_eq!(mixed_profile().jobs, 0.9);
    }

    #[test]
    fn initial_needs_are_stable_and_in_range() {
        assert_eq!(initial_need(SEED), initial_need(SEED));
        for seed in 0..200 {
            let need = initial_need(seed);
            assert!((0.35..=0.75).contains(&need), "seed {seed}: {need}");
        }
    }

    #[test]
    fn identical_states_have_zero_delta() {
        let state = Metrics {
            population: 500,
            employed: 450,
            happiness: 0.8125,
            average_need: 0.7,
        };
        let deltas = compare(&state, &state);
        assert_eq!(deltas.population, 0);
        assert_eq!(deltas.employed, 0);
        assert_eq!(deltas.max_delta(), 0.0);
    }

    #[test]
    fn delta_helper_normalizes_counts() {
        let reference = Metrics {
            population: 500,
            employed: 500,
            happiness: 0.8,
            average_need: 0.6,
        };
        let other = Metrics {
            population: 500,
            employed: 460,
            happiness: 0.792,
            average_need: 0.6,
        };
        let deltas = compare(&reference, &other);
        assert_eq!(deltas.population, 0);
        assert_eq!(deltas.employed, 40);
        assert!((deltas.employed_fraction - 0.08).abs() < 1e-12);
        assert!((deltas.happiness - 0.008).abs() < 1e-6);
        assert_eq!(deltas.average_need, 0.0);
        assert!((deltas.max_delta() - 0.08).abs() < 1e-12);
    }

    #[test]
    fn strict_comparison_matches_within_tolerance() {
        let outcome = simulate(small());
        assert_eq!(
            outcome.strict_aggregate.population,
            outcome.strict_individual.population
        );
        assert_eq!(
            outcome.strict_aggregate.employed,
            outcome.strict_individual.employed
        );
        assert!(outcome.strict_deltas.max_delta() <= STRICT_TOLERANCE);
        assert_eq!(outcome.strict_deltas.population, 0);
        assert_eq!(outcome.strict_deltas.employed, 0);
        // The per-citizen state is exact; the run-level residual is the
        // population-weighted f32 reduction order.
        assert_eq!(outcome.per_cohort_max_delta, 0.0);
        assert!(outcome.strict_deltas.average_need < 1e-5);
        assert!(outcome.strict_deltas.happiness < 1e-5);
    }

    #[test]
    fn coarse_and_fine_diverge_only_through_employment() {
        let outcome = simulate(small());
        assert_eq!(outcome.dt_deltas.population, 0);
        assert!(outcome.dt_deltas.employed_fraction > 0.0);
        // Needs otherwise converge to the same fixed point; allow f32
        // integration/reduction noise.
        assert!(outcome.dt_deltas.average_need < 1e-4);
        assert!(outcome.dt_deltas.happiness < 0.01);
        assert!(outcome.dt_deltas.employed > 0);
        assert!(!report(&outcome)["dt"]["step_invariant"]
            .as_bool()
            .expect("bool"));
        assert!(outcome.dt_deltas.max_delta() <= EMPLOYMENT_BOUND);
    }

    #[test]
    fn structural_delta_is_the_rounding_of_one_citizen_employment() {
        let outcome = simulate(small());
        assert_eq!(outcome.structural_deltas.population, 0);
        assert!(outcome.structural_deltas.average_need < 1e-5);
        assert_eq!(outcome.structural_individual.employed, 500);
        assert!(outcome.structural_deltas.employed > 0);
        assert!(outcome.structural_deltas.max_delta() <= EMPLOYMENT_BOUND);
    }

    #[test]
    fn default_scene_is_deterministic_and_passes() {
        let first = simulate(Config::default());
        let second = simulate(Config::default());
        assert_eq!(first.checksum, second.checksum);
        assert!(same_metrics(
            &first.strict_aggregate,
            &second.strict_aggregate
        ));
        assert!(same_metrics(&first.fine, &second.fine));
        assert!(same_deltas(&first.strict_deltas, &second.strict_deltas));
        assert!(same_deltas(&first.dt_deltas, &second.dt_deltas));
        assert!(same_deltas(
            &first.structural_deltas,
            &second.structural_deltas
        ));
        assert!(first.ok, "default scene must pass its gates");
    }

    #[test]
    fn report_carries_the_expected_keys() {
        let outcome = simulate(small());
        let report = report(&outcome);
        assert_eq!(report["scene"], "cohort_validate");
        assert_eq!(report["days"], 30);
        assert_eq!(report["status"], "ok");
        assert_eq!(report["population"], 500);
        assert!(report["aggregate"]["happiness"].is_number());
        assert!(report["dt"]["coarse"]["employed"].is_number());
        assert!(report["structural"]["individual"]["employed"].is_number());
        assert!(report["checksum"]
            .as_str()
            .is_some_and(|value| value.len() == 16));
    }
}
