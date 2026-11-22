//! Cohort-scale city simulation.

use std::error::Error;
use std::fmt;

use crate::cohort::Cohort;
use crate::district::DistrictServices;

/// Longest dormant catch-up one call may run, in whole days (ten years).
pub const MAX_CATCH_UP_DAYS: u32 = 3_650;

/// Simulation errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimError {
    /// A cohort referenced a district that does not exist.
    UnknownDistrict(u32),
    /// Catch-up was called with no days to advance.
    ZeroCatchUpDays,
    /// Catch-up requested more days than [`MAX_CATCH_UP_DAYS`].
    CatchUpDaysExceedCap {
        /// Requested days.
        requested: u32,
        /// Allowed maximum.
        cap: u32,
    },
}

impl fmt::Display for SimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownDistrict(id) => write!(f, "cohort references unknown district {id}"),
            Self::ZeroCatchUpDays => write!(f, "catch-up requires at least one day"),
            Self::CatchUpDaysExceedCap { requested, cap } => {
                write!(f, "catch-up requested {requested} days, cap is {cap}")
            }
        }
    }
}

impl Error for SimError {}

/// A population snapshot of one district's cohorts.
///
/// Averages are population-weighted and `0.0` when the district is empty.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CatchUpState {
    /// Citizens in the district.
    pub population: u64,
    /// Employed citizens.
    pub employed: u64,
    /// Population-weighted average happiness, `0..=1`.
    pub average_happiness: f32,
    /// Population-weighted average need satisfaction, `0..=1`.
    pub average_need: f32,
}

/// Result of one dormant-district catch-up call.
///
/// `steps` is the number of whole-day steps applied to each cohort and is `0`
/// for a district with no cohorts. `checksum` hashes the district index, the
/// day count, and every cohort in the district after the run, so it is a
/// deterministic fingerprint for save/benchmark comparisons.
#[derive(Debug, Clone, PartialEq)]
pub struct CatchUpReport {
    /// District index.
    pub district: u32,
    /// Whole days advanced.
    pub days: u32,
    /// Whole-day steps applied per cohort; `0` when the district is empty.
    pub steps: u32,
    /// Cohort records in the district.
    pub cohorts: u32,
    /// State before the run.
    pub before: CatchUpState,
    /// State after the run.
    pub after: CatchUpState,
    /// FNV-1a checksum of the final district state.
    pub checksum: u64,
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
/// Domain tag for catch-up checksums: "CATCHUP0".
const CATCH_UP_DOMAIN: u64 = 0x4341_5443_4855_5030;

fn fnv_u64(mut state: u64, value: u64) -> u64 {
    for byte in value.to_le_bytes() {
        state ^= u64::from(byte);
        state = state.wrapping_mul(FNV_PRIME);
    }
    state
}

fn validate_catch_up_days(days: u32) -> Result<(), SimError> {
    if days == 0 {
        return Err(SimError::ZeroCatchUpDays);
    }
    if days > MAX_CATCH_UP_DAYS {
        return Err(SimError::CatchUpDaysExceedCap {
            requested: days,
            cap: MAX_CATCH_UP_DAYS,
        });
    }
    Ok(())
}

/// A cohort simulation with per-district service inputs.
#[derive(Debug, Clone, PartialEq)]
pub struct CohortSimulation {
    districts: Vec<DistrictServices>,
    cohorts: Vec<Cohort>,
}

impl CohortSimulation {
    /// Create a simulation with `district_count` districts using default services.
    #[must_use]
    pub fn new(district_count: usize) -> Self {
        Self {
            districts: vec![DistrictServices::default(); district_count],
            cohorts: Vec::new(),
        }
    }

    /// Add a cohort. Returns an error if its district does not exist.
    pub fn add_cohort(&mut self, cohort: Cohort) -> Result<(), SimError> {
        if cohort.district as usize >= self.districts.len() {
            return Err(SimError::UnknownDistrict(cohort.district));
        }
        self.cohorts.push(cohort);
        Ok(())
    }

    /// Total population across all cohorts.
    #[must_use]
    pub fn total_population(&self) -> u64 {
        self.cohorts
            .iter()
            .map(|cohort| u64::from(cohort.count))
            .sum()
    }

    /// Total employed citizens.
    #[must_use]
    pub fn employed_population(&self) -> u64 {
        self.cohorts
            .iter()
            .map(|cohort| u64::from(cohort.employed))
            .sum()
    }

    /// Population-weighted average happiness, 0..=1.
    #[must_use]
    pub fn average_happiness(&self) -> f32 {
        let total = self.total_population();
        if total == 0 {
            return 0.0;
        }
        let weighted: f32 = self
            .cohorts
            .iter()
            .map(|cohort| cohort.happiness * cohort.count as f32)
            .sum();
        weighted / total as f32
    }

    /// Population-weighted average need satisfaction, 0..=1.
    #[must_use]
    pub fn average_need(&self) -> f32 {
        let total = self.total_population();
        if total == 0 {
            return 0.0;
        }
        let weighted: f32 = self
            .cohorts
            .iter()
            .map(|cohort| cohort.average_need() * cohort.count as f32)
            .sum();
        weighted / total as f32
    }

    /// Advance every cohort by `dt_days`.
    pub fn tick(&mut self, dt_days: f32) {
        for cohort in &mut self.cohorts {
            let services = self.districts[cohort.district as usize];
            cohort.tick(&services, dt_days);
        }
    }

    /// Advance one dormant district by `days` whole days.
    ///
    /// Only cohorts assigned to `district` move. Other districts and the
    /// district service inputs are left untouched. Steps are always exactly
    /// `1.0` days, which is arithmetic-identical to repeated [`Self::tick`]`(1.0)`
    /// calls for the same cohorts; because the employment carry round-trips
    /// exactly across whole days, `days` steps reproduce the continuously
    /// ticked state bit-for-bit.
    ///
    /// # Errors
    ///
    /// Returns [`SimError::UnknownDistrict`] if the district does not exist,
    /// [`SimError::ZeroCatchUpDays`] for `days == 0`, and
    /// [`SimError::CatchUpDaysExceedCap`] above [`MAX_CATCH_UP_DAYS`].
    pub fn catch_up(&mut self, district: u32, days: u32) -> Result<CatchUpReport, SimError> {
        if district as usize >= self.districts.len() {
            return Err(SimError::UnknownDistrict(district));
        }
        validate_catch_up_days(days)?;
        Ok(self.catch_up_validated(district, days))
    }

    /// Advance every district by `days` whole days.
    ///
    /// This is the multi-district convenience form of [`Self::catch_up`]. An
    /// empty district reports `steps = 0`. The result is equivalent to `days`
    /// calls to [`Self::tick`]`(1.0)`.
    ///
    /// # Errors
    ///
    /// Returns [`SimError::ZeroCatchUpDays`] for `days == 0` and
    /// [`SimError::CatchUpDaysExceedCap`] above [`MAX_CATCH_UP_DAYS`].
    pub fn catch_up_all(&mut self, days: u32) -> Result<Vec<CatchUpReport>, SimError> {
        validate_catch_up_days(days)?;
        let mut reports = Vec::with_capacity(self.districts.len());
        for district in 0..self.districts.len() as u32 {
            reports.push(self.catch_up_validated(district, days));
        }
        Ok(reports)
    }

    fn catch_up_validated(&mut self, district: u32, days: u32) -> CatchUpReport {
        let before = self.district_state(district);
        let cohorts = self
            .cohorts
            .iter()
            .filter(|cohort| cohort.district == district)
            .count() as u32;
        let steps = if cohorts == 0 { 0 } else { days };
        let services = self.districts[district as usize];

        for _ in 0..steps {
            for cohort in &mut self.cohorts {
                if cohort.district == district {
                    cohort.tick(&services, 1.0);
                }
            }
        }

        let after = self.district_state(district);
        let checksum = self.district_checksum(district, days);
        CatchUpReport {
            district,
            days,
            steps,
            cohorts,
            before,
            after,
            checksum,
        }
    }

    fn district_state(&self, district: u32) -> CatchUpState {
        let mut population = 0u64;
        let mut employed = 0u64;
        let mut happiness = 0f64;
        let mut need = 0f64;
        for cohort in self
            .cohorts
            .iter()
            .filter(|cohort| cohort.district == district)
        {
            population += u64::from(cohort.count);
            employed += u64::from(cohort.employed);
            let weight = f64::from(cohort.count);
            happiness += f64::from(cohort.happiness) * weight;
            need += f64::from(cohort.average_need()) * weight;
        }

        if population == 0 {
            return CatchUpState {
                population,
                employed,
                average_happiness: 0.0,
                average_need: 0.0,
            };
        }
        let weight = population as f64;
        CatchUpState {
            population,
            employed,
            average_happiness: (happiness / weight) as f32,
            average_need: (need / weight) as f32,
        }
    }

    fn district_checksum(&self, district: u32, days: u32) -> u64 {
        let mut state = fnv_u64(FNV_OFFSET, CATCH_UP_DOMAIN);
        state = fnv_u64(state, u64::from(district));
        state = fnv_u64(state, u64::from(days));
        for cohort in self
            .cohorts
            .iter()
            .filter(|cohort| cohort.district == district)
        {
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

    /// Read-only cohorts.
    #[must_use]
    pub fn cohorts(&self) -> &[Cohort] {
        &self.cohorts
    }

    /// Read-only districts.
    #[must_use]
    pub fn districts(&self) -> &[DistrictServices] {
        &self.districts
    }

    /// Mutable districts, for changing services or policy.
    pub fn districts_mut(&mut self) -> &mut [DistrictServices] {
        &mut self.districts
    }

    /// Number of cohorts.
    #[must_use]
    pub fn cohort_count(&self) -> usize {
        self.cohorts.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn city() -> CohortSimulation {
        let mut sim = CohortSimulation::new(4);
        for index in 0..100 {
            sim.add_cohort(Cohort::new((index % 4) as u32, 10_000, 0.8))
                .expect("district exists");
        }
        sim
    }

    #[test]
    fn builds_a_million_citizen_city() {
        let sim = city();
        assert_eq!(sim.total_population(), 1_000_000);
        assert_eq!(sim.cohort_count(), 100);
        assert_eq!(sim.employed_population(), 1_000_000);
    }

    #[test]
    fn ticking_preserves_population_and_updates_happiness() {
        let mut sim = city();
        sim.tick(1.0);
        assert_eq!(sim.total_population(), 1_000_000);
        assert!((0.0..=1.0).contains(&sim.average_happiness()));
        assert!((0.0..=1.0).contains(&sim.average_need()));
    }

    #[test]
    fn bad_services_lower_happiness() {
        let mut good = CohortSimulation::new(1);
        good.add_cohort(Cohort::new(0, 1_000, 0.9))
            .expect("district");
        let mut bad = CohortSimulation::new(1);
        bad.add_cohort(Cohort::new(0, 1_000, 0.9))
            .expect("district");
        bad.districts_mut()[0] = DistrictServices {
            food: 0.0,
            water: 0.0,
            jobs: 0.0,
            pollution: 1.0,
            ..DistrictServices::default()
        };

        for _ in 0..20 {
            good.tick(1.0);
            bad.tick(1.0);
        }
        assert!(good.average_happiness() > bad.average_happiness());
    }

    #[test]
    fn unknown_districts_are_rejected() {
        let mut sim = CohortSimulation::new(1);
        assert_eq!(
            sim.add_cohort(Cohort::new(4, 10, 0.5)),
            Err(SimError::UnknownDistrict(4))
        );
    }

    #[test]
    fn zero_dt_does_not_change_the_city() {
        let mut sim = city();
        let before = sim.clone();
        sim.tick(0.0);
        assert_eq!(sim, before);
    }

    #[test]
    fn multi_day_city_tick_equals_repeated_day_ticks() {
        let mut split = single_district_city();
        let mut repeated = split.clone();

        split.tick(7.0);
        for _ in 0..7 {
            repeated.tick(1.0);
        }

        assert_eq!(split, repeated);
    }

    fn district_cohorts(sim: &CohortSimulation, district: u32) -> Vec<Cohort> {
        sim.cohorts()
            .iter()
            .filter(|cohort| cohort.district == district)
            .cloned()
            .collect()
    }

    fn single_district_city() -> CohortSimulation {
        let mut sim = CohortSimulation::new(1);
        sim.districts_mut()[0] = DistrictServices {
            jobs: 0.9,
            ..DistrictServices::default()
        };
        for index in 0..5 {
            let need = 0.35 + index as f32 * 0.1;
            sim.add_cohort(Cohort::new(0, 100, need))
                .expect("district exists");
        }
        sim
    }

    #[test]
    fn catch_up_advances_only_the_target_district() {
        let mut sim = city();
        let before = sim.clone();
        let report = sim.catch_up(1, 30).expect("district exists");

        for district in [0u32, 2, 3] {
            assert_eq!(
                district_cohorts(&sim, district),
                district_cohorts(&before, district),
                "district {district} must not move"
            );
        }
        assert_ne!(
            district_cohorts(&sim, 1),
            district_cohorts(&before, 1),
            "the target district must advance"
        );
        assert_eq!(sim.districts(), before.districts());
        assert_eq!(report.district, 1);
        assert_eq!(report.days, 30);
        assert_eq!(report.steps, 30);
        assert_eq!(report.cohorts, 25);
        assert_eq!(report.after.population, 250_000);
        assert_eq!(report.after, sim.district_state(1));
    }

    #[test]
    fn one_year_catch_up_equals_repeated_day_ticks() {
        let mut dormant = single_district_city();
        let mut continuous = dormant.clone();

        let report = dormant.catch_up(0, 365).expect("district exists");
        for _ in 0..365 {
            continuous.tick(1.0);
        }

        assert_eq!(dormant, continuous);
        assert_eq!(report.steps, 365);
        assert_eq!(report.after, dormant.district_state(0));
        assert_eq!(report.before.population, report.after.population);
    }

    #[test]
    fn catch_up_all_equals_repeated_day_ticks() {
        let mut dormant = city();
        let mut continuous = dormant.clone();

        let reports = dormant.catch_up_all(90).expect("valid days");
        for _ in 0..90 {
            continuous.tick(1.0);
        }

        assert_eq!(dormant, continuous);
        assert_eq!(reports.len(), 4);
        for (district, report) in reports.iter().enumerate() {
            assert_eq!(report.district, district as u32);
            assert_eq!(report.days, 90);
            assert_eq!(report.steps, 90);
            assert_eq!(report.after, dormant.district_state(district as u32));
        }
    }

    #[test]
    fn catch_up_rejects_zero_unknown_and_over_cap_days() {
        let mut sim = city();

        assert_eq!(sim.catch_up(9, 1), Err(SimError::UnknownDistrict(9)));
        assert_eq!(sim.catch_up(0, 0), Err(SimError::ZeroCatchUpDays));
        assert_eq!(
            sim.catch_up(0, MAX_CATCH_UP_DAYS + 1),
            Err(SimError::CatchUpDaysExceedCap {
                requested: MAX_CATCH_UP_DAYS + 1,
                cap: MAX_CATCH_UP_DAYS,
            })
        );
        assert!(sim.catch_up_all(0).is_err());
        assert!(sim.catch_up_all(MAX_CATCH_UP_DAYS + 1).is_err());

        let before = sim.clone();
        let report = sim
            .catch_up(0, MAX_CATCH_UP_DAYS)
            .expect("the cap itself is allowed");
        assert_eq!(report.steps, MAX_CATCH_UP_DAYS);
        assert_ne!(sim, before);
    }

    #[test]
    fn catch_up_checksum_is_deterministic() {
        let mut first = city();
        let mut second = city();
        let first_report = first.catch_up(2, 365).expect("district exists");
        let second_report = second.catch_up(2, 365).expect("district exists");

        assert_eq!(first_report, second_report);
        assert_eq!(first_report.checksum, second_report.checksum);

        let mut shorter = city();
        let shorter_report = shorter.catch_up(2, 364).expect("district exists");
        assert_ne!(first_report.checksum, shorter_report.checksum);

        let mut other_district = city();
        let other_report = other_district.catch_up(3, 365).expect("district exists");
        assert_ne!(first_report.checksum, other_report.checksum);
    }

    #[test]
    fn catch_up_preserves_population_and_value_bounds() {
        let mut sim = city();
        let report = sim.catch_up(0, 365).expect("district exists");

        assert_eq!(report.before.population, 250_000);
        assert_eq!(report.after.population, report.before.population);
        assert!(report.after.employed <= report.after.population);
        assert!((0.0..=1.0).contains(&report.after.average_happiness));
        assert!((0.0..=1.0).contains(&report.after.average_need));
        assert_eq!(report.before.employed, 250_000);
        assert!(report.after.employed < report.before.employed);
    }

    #[test]
    fn catch_up_on_an_empty_district_is_a_no_op() {
        let mut sim = CohortSimulation::new(2);
        sim.add_cohort(Cohort::new(1, 100, 0.5))
            .expect("district exists");
        let before = sim.clone();

        let report = sim.catch_up(0, 30).expect("district exists");
        assert_eq!(sim, before);
        assert_eq!(report.cohorts, 0);
        assert_eq!(report.steps, 0);
        assert_eq!(report.before, CatchUpState::default());
        assert_eq!(report.before, report.after);

        let reports = sim.catch_up_all(30).expect("valid days");
        assert_eq!(reports[0].steps, 0);
        assert_eq!(reports[1].steps, 30);
    }
}
