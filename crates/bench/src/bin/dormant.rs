//! Dormant-region catch-up validation.
//!
//! One small multi-district city is left dormant for a year, then advanced
//! with [`CohortSimulation::catch_up`]. A second copy is advanced by continuous
//! `tick(1.0)` calls. Catch-up must reproduce the continuous state exactly:
//! whole-day catch-up steps are the same arithmetic as continuous daily
//! ticking, so the state is bit-identical. (Fractional-day steps now converge
//! too, thanks to the employment carry, but whole days remain the catch-up rule
//! so the result is exactly the continuous one.)
//!
//! The scene reports:
//!
//! - target-district state deltas between catch-up and continuous ticking;
//! - that every other district is bit-for-bit unchanged;
//! - the `catch_up_all(365)` convenience against continuous ticking;
//! - a deterministic FNV checksum over the final city.
//!
//! `--smoke` runs the scene twice and fails on any drift. There are no sleeps
//! and no wall-clock measurements.
//!
//! See `docs/07 - Simulation and AI.md` §3 and §14.

use core_sim::{CatchUpState, Cohort, CohortSimulation, DistrictServices, MAX_CATCH_UP_DAYS};

/// Scene seed; salts initial needs and the checksum. No random process runs.
const SEED: u64 = 0x0044_4F52_4D41_4E54; // "DORMANT"
/// Dormant days per run: one game year.
const DEFAULT_DAYS: u32 = 365;
/// Districts in the scene.
const DISTRICTS: usize = 4;
/// Cohorts per district.
const COHORTS_PER_DISTRICT: usize = 5;
/// Citizens per cohort.
const CITIZENS_PER_COHORT: u32 = 100;
/// The dormant district under test.
const TARGET_DISTRICT: u32 = 0;
/// Acceptance bound on the state delta. Catch-up is the same whole-day
/// arithmetic as continuous ticking, so this must be exactly zero.
const MAX_DELTA_BOUND: f64 = 0.0;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
/// Domain tag: "DORMANT".
const DOMAIN: u64 = 0x0044_4F52_4D41_4E54;

/// The four comparable city metrics.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Metrics {
    population: u64,
    employed: u64,
    happiness: f32,
    average_need: f32,
}

impl Metrics {
    fn from_state(state: &CatchUpState) -> Self {
        Self {
            population: state.population,
            employed: state.employed,
            happiness: state.average_happiness,
            average_need: state.average_need,
        }
    }

    fn for_district(city: &CohortSimulation, district: u32) -> Self {
        let summary = city.district_summary(district).expect("district exists");
        Self {
            population: summary.population,
            employed: summary.employed,
            happiness: summary.average_happiness,
            average_need: summary.average_need,
        }
    }

    fn employed_fraction(&self) -> f64 {
        if self.population == 0 {
            return 0.0;
        }
        self.employed as f64 / self.population as f64
    }
}

/// Absolute divergence of `other` from `reference`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Deltas {
    population: u64,
    employed: u64,
    happiness: f64,
    average_need: f64,
}

impl Deltas {
    fn max_delta(&self) -> f64 {
        self.happiness.max(self.average_need)
    }
}

fn compare(reference: &Metrics, other: &Metrics) -> Deltas {
    Deltas {
        population: reference.population.abs_diff(other.population),
        employed: reference.employed.abs_diff(other.employed),
        happiness: (f64::from(reference.happiness) - f64::from(other.happiness)).abs(),
        average_need: (f64::from(reference.average_need) - f64::from(other.average_need)).abs(),
    }
}

/// Final state of every run in the scene.
#[derive(Debug)]
struct Outcome {
    days: u32,
    population: u64,
    dormant: Metrics,
    continuous: Metrics,
    deltas: Deltas,
    other_districts_unchanged: bool,
    all_max_delta: f64,
    all_checksums_match: bool,
    report_checksum: u64,
    city_checksum: u64,
    ok: bool,
}

fn main() {
    if std::env::args().any(|arg| arg == "--smoke") {
        run_smoke();
        return;
    }

    let outcome = simulate(DEFAULT_DAYS);
    println!(
        "{}",
        serde_json::to_string(&report(&outcome)).expect("json")
    );
    if !outcome.ok {
        std::process::exit(1);
    }
}

fn run_smoke() {
    let first = simulate(DEFAULT_DAYS);
    let second = simulate(DEFAULT_DAYS);
    let deterministic = first.report_checksum == second.report_checksum
        && first.city_checksum == second.city_checksum
        && same_metrics(&first.dormant, &second.dormant)
        && same_metrics(&first.continuous, &second.continuous)
        && same_deltas(&first.deltas, &second.deltas)
        && first.all_max_delta.to_bits() == second.all_max_delta.to_bits();
    let ok = deterministic && first.ok;

    println!(
        concat!(
            "DORMANT_SMOKE days={} population={} target_delta={:.1e} ",
            "other_districts_unchanged={} catch_up_all_max_delta={:.1e} ",
            "report_checksum={:016x} city_checksum={:016x} ",
            "deterministic={} status={}"
        ),
        first.days,
        first.population,
        first.deltas.max_delta(),
        first.other_districts_unchanged,
        first.all_max_delta,
        first.report_checksum,
        first.city_checksum,
        deterministic,
        if ok { "ok" } else { "failed" }
    );
    if !ok {
        std::process::exit(1);
    }
}

fn simulate(days: u32) -> Outcome {
    assert!(
        days > 0 && days <= MAX_CATCH_UP_DAYS,
        "scene days must be within the catch-up cap"
    );

    let baseline = build_city();
    let mut dormant = baseline.clone();
    let mut continuous = baseline.clone();
    let mut all_dormant = baseline.clone();

    let catch_up = dormant
        .catch_up(TARGET_DISTRICT, days)
        .expect("target district exists");
    for _ in 0..days {
        continuous.tick(1.0);
    }
    let all_reports = all_dormant.catch_up_all(days).expect("valid day count");

    let dormant_metrics = Metrics::from_state(&catch_up.after);
    let continuous_metrics = Metrics::for_district(&continuous, TARGET_DISTRICT);
    let deltas = compare(&continuous_metrics, &dormant_metrics);

    let other_districts_unchanged = (1..DISTRICTS as u32).all(|district| {
        city_district_checksum(&dormant, district) == city_district_checksum(&baseline, district)
    });

    let all_max_delta = (0..DISTRICTS as u32)
        .map(|district| {
            compare(
                &Metrics::for_district(&continuous, district),
                &Metrics::for_district(&all_dormant, district),
            )
            .max_delta()
        })
        .fold(0.0_f64, f64::max);
    let all_checksums_match = city_checksum(&all_dormant) == city_checksum(&continuous);

    let population = dormant.total_population();
    let ok = deltas.max_delta() <= MAX_DELTA_BOUND
        && deltas.population == 0
        && deltas.employed == 0
        && catch_up.steps == days
        && catch_up.cohorts as usize == COHORTS_PER_DISTRICT
        && catch_up.before.population == catch_up.after.population
        && other_districts_unchanged
        && all_max_delta <= MAX_DELTA_BOUND
        && all_checksums_match
        && all_reports.len() == DISTRICTS
        && all_reports.iter().all(|report| {
            report.days == days
                && report.steps == days
                && report.cohorts as usize == COHORTS_PER_DISTRICT
                && report.before.population == report.after.population
        });

    Outcome {
        days,
        population,
        dormant: dormant_metrics,
        continuous: continuous_metrics,
        deltas,
        other_districts_unchanged,
        all_max_delta,
        all_checksums_match,
        report_checksum: catch_up.checksum,
        city_checksum: city_checksum(&dormant),
        ok,
    }
}

/// Four districts with distinct service profiles, so a leaked cohort tick from
/// one district into another would be visible.
fn profile(district: usize) -> DistrictServices {
    let mut services = DistrictServices::default();
    match district {
        0 => {
            // Mixed employment: the profile that exposes step sensitivity.
            services.jobs = 0.9;
        }
        1 => {
            services.food = 0.75;
            services.water = 0.8;
            services.jobs = 0.85;
            services.pollution = 0.35;
        }
        2 => {
            services.social = 0.9;
            services.jobs = 0.95;
            services.wages = 1.2;
        }
        _ => {
            services.food = 0.6;
            services.safety = 0.55;
            services.jobs = 0.7;
            services.commute = 0.4;
        }
    }
    services
}

fn initial_need(district: usize, cohort: usize) -> f32 {
    let seed = (SEED as usize).wrapping_add(district * 7 + cohort * 3);
    0.35 + (seed % 40) as f32 * 0.01
}

fn build_city() -> CohortSimulation {
    let mut city = CohortSimulation::new(DISTRICTS);
    for district in 0..DISTRICTS {
        city.districts_mut()[district] = profile(district);
        for cohort in 0..COHORTS_PER_DISTRICT {
            city.add_cohort(Cohort::new(
                district as u32,
                CITIZENS_PER_COHORT,
                initial_need(district, cohort),
            ))
            .expect("district exists");
        }
    }
    city
}

fn report(outcome: &Outcome) -> serde_json::Value {
    serde_json::json!({
        "scene": "dormant",
        "format_version": 1,
        "seed": SEED,
        "days": outcome.days,
        "districts": DISTRICTS,
        "cohorts_per_district": COHORTS_PER_DISTRICT,
        "citizens_per_cohort": CITIZENS_PER_COHORT,
        "population": outcome.population,
        "target_district": TARGET_DISTRICT,
        "target_population": outcome.dormant.population,
        "dormant": metrics_json(&outcome.dormant),
        "continuous": metrics_json(&outcome.continuous),
        "deltas": deltas_json(&outcome.deltas),
        "max_delta": outcome.deltas.max_delta(),
        "tolerance": MAX_DELTA_BOUND,
        "other_districts_unchanged": outcome.other_districts_unchanged,
        "catch_up_all": {
            "days": outcome.days,
            "districts": DISTRICTS,
            "max_delta": outcome.all_max_delta,
            "checksums_match": outcome.all_checksums_match,
        },
        "report_checksum": format!("{:016x}", outcome.report_checksum),
        "city_checksum": format!("{:016x}", outcome.city_checksum),
        "checksum_scope": "report: district + days + target cohorts; city: every cohort of the full city",
        "step_rule": "whole 1.0-day steps; arithmetic-identical to continuous daily ticking (fractional steps also converge now that employment carries a fractional remainder)",
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
        "happiness": deltas.happiness,
        "average_need": deltas.average_need,
        "max_delta": deltas.max_delta(),
    })
}

fn city_district_checksum(city: &CohortSimulation, district: u32) -> u64 {
    let mut state = fnv_u64(FNV_OFFSET, DOMAIN);
    state = fnv_u64(state, u64::from(district));
    for cohort in city
        .cohorts()
        .iter()
        .filter(|cohort| cohort.district == district)
    {
        state = cohort_checksum(state, cohort);
    }
    state
}

fn city_checksum(city: &CohortSimulation) -> u64 {
    let mut state = fnv_u64(FNV_OFFSET, DOMAIN);
    state = fnv_u64(state, city.cohort_count() as u64);
    for cohort in city.cohorts() {
        state = cohort_checksum(state, cohort);
    }
    state
}

fn cohort_checksum(mut state: u64, cohort: &Cohort) -> u64 {
    state = fnv_u64(state, u64::from(cohort.district));
    state = fnv_u64(state, u64::from(cohort.count));
    state = fnv_u64(state, u64::from(cohort.employed));
    for need in cohort.needs {
        state = fnv_u64(state, u64::from(need.to_bits()));
    }
    state = fnv_u64(state, u64::from(cohort.happiness.to_bits()));
    fnv_u64(state, u64::from(cohort.income.to_bits()))
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
        && left.happiness.to_bits() == right.happiness.to_bits()
        && left.average_need.to_bits() == right.average_need.to_bits()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_shape_is_fixed() {
        let city = build_city();
        assert_eq!(city.cohort_count(), DISTRICTS * COHORTS_PER_DISTRICT);
        assert_eq!(
            city.total_population(),
            (DISTRICTS * COHORTS_PER_DISTRICT) as u64 * u64::from(CITIZENS_PER_COHORT)
        );
        assert_eq!(city.districts().len(), DISTRICTS);
        assert_eq!(city.districts()[0].jobs, 0.9);
    }

    #[test]
    fn one_year_catch_up_matches_continuous_ticking() {
        let outcome = simulate(DEFAULT_DAYS);
        assert_eq!(outcome.deltas.population, 0);
        assert_eq!(outcome.deltas.employed, 0);
        assert_eq!(outcome.deltas.max_delta(), 0.0);
        assert!(outcome.other_districts_unchanged);
        assert!(outcome.all_checksums_match);
        assert_eq!(outcome.all_max_delta, 0.0);
        assert!(outcome.ok);
    }

    #[test]
    fn scene_is_deterministic() {
        let first = simulate(DEFAULT_DAYS);
        let second = simulate(DEFAULT_DAYS);
        assert_eq!(first.report_checksum, second.report_checksum);
        assert_eq!(first.city_checksum, second.city_checksum);
        assert!(same_metrics(&first.dormant, &second.dormant));
        assert!(same_deltas(&first.deltas, &second.deltas));
    }

    #[test]
    fn shorter_absence_has_a_different_checksum() {
        let year = simulate(DEFAULT_DAYS);
        let month = simulate(30);
        assert_ne!(year.report_checksum, month.report_checksum);
        assert_ne!(year.city_checksum, month.city_checksum);
    }

    #[test]
    fn report_carries_the_expected_keys() {
        let report = report(&simulate(DEFAULT_DAYS));
        assert_eq!(report["scene"], "dormant");
        assert_eq!(report["status"], "ok");
        assert_eq!(report["population"], 2_000);
        assert_eq!(report["target_population"], 500);
        assert_eq!(report["deltas"]["max_delta"], 0.0);
        assert_eq!(report["catch_up_all"]["checksums_match"], true);
        assert!(report["city_checksum"]
            .as_str()
            .is_some_and(|value| value.len() == 16));
    }
}
